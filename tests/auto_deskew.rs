//! Auto-deskew end to end (zenpipe#27): a ruled image skewed by the
//! pipeline's own `RotateEffect` is straightened by `AutoDeskewEffect`,
//! resolved on the materialized frame inside `EffectSource`.

use zenlayout::dimension::{AutoDeskewEffect, DimensionEffect, RotateEffect, RotateMode};
use zenlayout::{ResolvedEffect, Size};
use zenpipe::limits::Limits;
use zenpipe::sources::{EffectSource, MaterializedSource};
use zenpipe::{Source, format};

/// White RGBA8 with anti-aliased dark horizontal rules (3 px thick, 16 px apart).
fn ruled_rgba(w: u32, h: u32) -> Box<dyn Source> {
    let mut data = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        let d = (y as f32 - h as f32 / 2.0).rem_euclid(16.0);
        let cov = (d + 0.5).min(3.5 - d).clamp(0.0, 1.0);
        let v = (255.0 * (1.0 - cov)).round() as u8;
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            data[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    Box::new(MaterializedSource::from_data(
        data,
        w,
        h,
        format::RGBA8_SRGB,
    ))
}

fn drain_luma(src: &mut dyn Source) -> (Vec<u8>, u32, u32) {
    let (w, h) = (src.width(), src.height());
    let mut luma = Vec::with_capacity((w * h) as usize);
    while let Some(strip) = src.next().unwrap() {
        for r in 0..strip.rows() {
            for px in strip.row(r)[..w as usize * 4].chunks_exact(4) {
                let a = u32::from(px[3]);
                let l =
                    (54 * u32::from(px[0]) + 183 * u32::from(px[1]) + 19 * u32::from(px[2])) >> 8;
                luma.push(((l * a + 255 * (255 - a)) / 255) as u8);
            }
        }
    }
    (luma, w, h)
}

fn rotate(src: Box<dyn Source>, deg: f32, mode: RotateMode) -> EffectSource {
    let (w, h) = (src.width(), src.height());
    let eff = RotateEffect::from_degrees(deg, mode);
    let (ow, oh) = eff.forward(w, h).unwrap();
    let re = ResolvedEffect {
        effect: Box::new(eff),
        input_dims: Size::new(w, h),
        output_dims: Size::new(ow, oh),
        command_index: 0,
        before_resize: true,
    };
    EffectSource::new(src, &[re], &Limits::default()).unwrap()
}

fn deskew(src: Box<dyn Source>, mode: RotateMode) -> EffectSource {
    let (w, h) = (src.width(), src.height());
    // An analysis barrier: the planner cannot know the output dims, so it
    // carries the input dims through; EffectSource recomputes them.
    let re = ResolvedEffect {
        effect: Box::new(AutoDeskewEffect::new(mode, 10.0)),
        input_dims: Size::new(w, h),
        output_dims: Size::new(w, h),
        command_index: 1,
        before_resize: true,
    };
    EffectSource::new(src, &[re], &Limits::default()).unwrap()
}

#[test]
fn skewed_rules_are_straightened_within_0_2_degrees() {
    for skew in [4.0f32, -3.0, 7.5] {
        // Skew with the pipeline's own rotation (bilinear, CropToOriginal).
        let skewed = rotate(ruled_rgba(320, 240), skew, RotateMode::CropToOriginal);
        assert_eq!((skewed.width(), skewed.height()), (320, 240));

        let mut fixed = deskew(Box::new(skewed), RotateMode::CropToOriginal);
        let resolved = fixed.effects();
        assert_eq!(
            resolved.len(),
            1,
            "barrier must resolve to one concrete effect"
        );
        let applied = resolved[0]
            .effect
            .rotation_angle_rad()
            .expect("AutoDeskewEffect resolves to a RotateEffect")
            .to_degrees();
        assert!(
            (applied + skew).abs() <= 0.2,
            "skew {skew}: applied rotation {applied} (want {})",
            -skew
        );
        assert!(resolved[0].effect.forward(320, 240).is_some());

        // The output is straight: the detector sees ≈0° on it.
        let (luma, w, h) = drain_luma(&mut fixed);
        let residual =
            zenlayout::deskew::detect_skew_projection_variance(&luma, w, h, w as usize, 10.0)
                .unwrap_or(0.0);
        assert!(residual.abs() <= 0.3, "skew {skew}: residual {residual}");
    }
}

#[test]
fn inscribed_crop_mode_shrinks_to_the_resolved_rotation() {
    let skewed = rotate(ruled_rgba(320, 240), 5.0, RotateMode::CropToOriginal);
    let fixed = deskew(Box::new(skewed), RotateMode::InscribedCrop);
    let expect = RotateEffect::from_degrees(-5.0, RotateMode::InscribedCrop)
        .forward(320, 240)
        .unwrap();
    // ±1 px slack for the ±0.2° detection tolerance.
    assert!(
        (fixed.width() as i64 - expect.0 as i64).abs() <= 1
            && (fixed.height() as i64 - expect.1 as i64).abs() <= 1,
        "got {}x{}, expected ≈{}x{}",
        fixed.width(),
        fixed.height(),
        expect.0,
        expect.1
    );
    assert!(fixed.width() < 320 && fixed.height() < 240);
}

#[test]
fn featureless_input_resolves_to_identity() {
    let flat = Box::new(MaterializedSource::from_data(
        vec![200u8; 64 * 48 * 4],
        64,
        48,
        format::RGBA8_SRGB,
    ));
    let fixed = deskew(flat, RotateMode::InscribedCrop);
    assert_eq!((fixed.width(), fixed.height()), (64, 48));
    let applied = fixed.effects()[0].effect.rotation_angle_rad().unwrap();
    assert_eq!(applied, 0.0);
}
