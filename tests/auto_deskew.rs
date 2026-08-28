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
            for px in strip.row(r)[..w as usize * 4].as_chunks::<4>().0.iter() {
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
    // And the pixels are straight, not just cropped.
    let mut fixed = fixed;
    let (luma, w, h) = drain_luma(&mut fixed);
    let residual =
        zenlayout::deskew::detect_skew_projection_variance(&luma, w, h, w as usize, 10.0)
            .unwrap_or(0.0);
    eprintln!("inscribed: {w}x{h} residual {residual}");
    assert!(residual.abs() <= 0.3, "inscribed residual {residual}");
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

/// The whole path a request takes: `?autodeskew=1&w=200` → registry →
/// IR4 ordering → geometry fusion (the barrier lands in the layout plan)
/// → graph compile (the plan is recomputed once the barrier resolves) →
/// straight, correctly-proportioned output.
#[cfg(feature = "zennode")]
#[test]
fn autodeskew_querystring_straightens_through_the_bridge() {
    let skew = 4.0f32;
    let skewed = rotate(ruled_rgba(320, 240), skew, RotateMode::CropToOriginal);
    let (w, h) = (skewed.width(), skewed.height());

    let registry = zenpipe::full_registry();
    let mut nodes = registry.from_querystring("autodeskew=1&w=200").instances;
    assert!(
        nodes
            .iter()
            .any(|n| n.schema().id == "zenlayout.auto_deskew"),
        "autodeskew key must create the node"
    );
    zenpipe::riapi::riapi_order(&mut nodes);
    let compiled = zenpipe::bridge::compile_nodes(&nodes, &[], w, h, None).unwrap();

    // Before execution the plan only knows the barrier's input dims: the
    // constraint was computed on 320×240, so the placeholder plan says
    // 200×150. The graph re-plans once the barrier resolves (checked below
    // through the output height).
    let mut sources = hashbrown::HashMap::new();
    sources.insert(0usize, Box::new(skewed) as Box<dyn Source>);
    let mut pipeline = compiled.graph.compile(sources).unwrap();

    // Inscribed crop of 320×240 at 4° is ≈ 296×212 (aspect 1.396), then
    // `w=200` → 200×143 — NOT the 200×150 the placeholder plan implied.
    let expect = RotateEffect::from_degrees(-skew, RotateMode::InscribedCrop)
        .forward(320, 240)
        .unwrap();
    let expect_h = (200.0 * expect.1 as f32 / expect.0 as f32).round() as i64;
    let (ow, oh) = (pipeline.width(), pipeline.height());
    assert_eq!(ow, 200);
    assert!(
        (oh as i64 - expect_h).abs() <= 2,
        "output {ow}x{oh}, expected 200x{expect_h} from the re-plan"
    );

    let (luma, lw, lh) = drain_luma(pipeline.as_mut());
    assert_eq!((lw, lh), (ow, oh));
    let residual =
        zenlayout::deskew::detect_skew_projection_variance(&luma, lw, lh, lw as usize, 10.0)
            .unwrap_or(0.0);
    assert!(residual.abs() <= 0.3, "residual skew {residual}");
}

/// A test barrier that resolves to a 45° expanding rotation, so the
/// planner's placeholder (input dims carried through) and the re-planned
/// result differ unmistakably: 320×240 → 396×396 → `w=200` gives 200×200,
/// where the placeholder plan said 200×150.
#[derive(Clone, Copy, Debug)]
struct Rotate45Barrier;

impl DimensionEffect for Rotate45Barrier {
    fn forward(&self, _w: u32, _h: u32) -> Option<(u32, u32)> {
        None
    }
    fn inverse(&self, _w: u32, _h: u32) -> Option<(u32, u32)> {
        None
    }
    fn forward_point(&self, _x: f32, _y: f32, _w: u32, _h: u32) -> Option<(f32, f32)> {
        None
    }
    fn inverse_point(&self, _x: f32, _y: f32, _w: u32, _h: u32) -> Option<(f32, f32)> {
        None
    }
    fn analyze(&self, _l: &[u8], _w: u32, _h: u32, _s: usize) -> Option<Box<dyn DimensionEffect>> {
        Some(Box::new(RotateEffect::from_degrees(
            45.0,
            RotateMode::Expand {
                color: zenlayout::CanvasColor::Transparent,
            },
        )))
    }
    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(*self)
    }
}

/// The graph re-plans a `NodeOp::Layout` whose plan carries an analysis
/// barrier (`LayoutPlan::replan`) once `EffectSource` resolves it.
#[test]
fn layout_replans_after_the_barrier_resolves() {
    use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};

    let pipeline = zenlayout::Pipeline::new(320, 240)
        .effect(Rotate45Barrier)
        .within(200, 10_000);
    assert!(pipeline.has_analysis_barrier());
    let (ideal, request) = pipeline.clone().plan().unwrap();
    let mut plan = ideal.finalize(&request, &zenlayout::DecoderOffer::full_decode(320, 240));
    assert_eq!(plan.effects.len(), 1, "the barrier is in the plan");
    assert_eq!(
        (plan.canvas.width, plan.canvas.height),
        (200, 150),
        "placeholder"
    );
    plan.replan = Some(pipeline);

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let layout = g.add_node(NodeOp::Layout {
        plan,
        filter: zenresize::Filter::Robidoux,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, layout, EdgeKind::Input);
    g.add_edge(layout, out, EdgeKind::Input);

    let mut sources = hashbrown::HashMap::new();
    sources.insert(src, ruled_rgba(320, 240));
    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(
        (pipeline.width(), pipeline.height()),
        (200, 200),
        "re-planned from the resolved 45° expand rotation (396×396)"
    );
    // Drains without error and the frame is the rotated content: the
    // expanded corners are transparent, so the luma-over-white top-left
    // corner is white while the centre carries the rules.
    let (luma, w, h) = drain_luma(pipeline.as_mut());
    assert_eq!((w, h), (200, 200));
    assert_eq!(luma[0], 255, "transparent corner over white");
    let centre_dark = (95..105)
        .flat_map(|y| (95..105).map(move |x| (x, y)))
        .any(|(x, y)| luma[y * 200 + x] < 128);
    assert!(centre_dark, "rotated rules reach the centre");
}
