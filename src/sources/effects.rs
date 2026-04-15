//! Apply a [`DimensionEffect`] (e.g. non-cardinal rotation, warp) to a source.
//!
//! Materializes the upstream source, then produces a rotated/warped buffer of
//! the effect's `output_dims`, and replays it as strips. Uses inverse-mapping
//! with bilinear interpolation in sRGB space. For RGBA8_SRGB sources only.
//!
//! The one materialization barrier is unavoidable — rotation reads from a
//! different location in the source grid for every output pixel.

use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
use crate::format::{self, PixelFormat};
use crate::limits::Limits;
use crate::strip::Strip;
use whereat::at;
use zenlayout::ResolvedEffect;

/// Apply a sequence of [`ResolvedEffect`] to an upstream source.
///
/// Currently supports inverse-mapping rotation via [`DimensionEffect::inverse_point`].
/// Each effect is applied in order; the output of one feeds the next.
///
/// Input/output format is RGBA8_SRGB. Interpolation is bilinear in sRGB (fast,
/// good enough for deskew angles; for higher quality, apply effects in a
/// linearized pipeline upstream).
pub struct EffectSource {
    data: alloc::vec::Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
    strip_height: u32,
    y: u32,
}

impl EffectSource {
    /// Drain `upstream`, apply each effect in order, yield strips from the result.
    ///
    /// Effects with `None` `forward()` (content-adaptive) are skipped — those
    /// require an analyze phase that isn't wired in yet.
    pub fn new(
        mut upstream: Box<dyn Source>,
        effects: &[ResolvedEffect],
        limits: &Limits,
    ) -> crate::PipeResult<Self> {
        // Materialize source.
        let width = upstream.width();
        let height = upstream.height();
        let fmt = upstream.format();
        if fmt != format::RGBA8_SRGB {
            return Err(at!(crate::error::PipeError::Op(alloc::string::String::from(
                "EffectSource requires RGBA8_SRGB input"
            ))));
        }
        limits.check(width, height, fmt)?;

        let row_bytes = fmt.aligned_stride(width);
        let mut data = vec![0u8; row_bytes * height as usize];
        let mut y = 0u32;
        while let Some(strip) = upstream.next()? {
            for r in 0..strip.rows() {
                let dst_start = (y + r) as usize * row_bytes;
                let src_row = strip.row(r);
                data[dst_start..dst_start + row_bytes].copy_from_slice(&src_row[..row_bytes]);
            }
            y += strip.rows();
        }

        let mut cur_w = width;
        let mut cur_h = height;
        let mut cur_data = data;

        for effect in effects {
            let (out_w, out_h) = (effect.output_dims.width, effect.output_dims.height);
            let in_w = effect.input_dims.width;
            let in_h = effect.input_dims.height;

            // Sanity: the effect's declared input dimensions must match our current buffer.
            if in_w != cur_w || in_h != cur_h {
                return Err(at!(crate::error::PipeError::Op(alloc::format!(
                    "EffectSource: effect input_dims ({}x{}) don't match current buffer ({}x{})",
                    in_w, in_h, cur_w, cur_h
                ))));
            }

            // Inverse-mapping: for every output pixel, find source coordinate and interpolate.
            let out_stride = (out_w as usize) * 4;
            let mut out = vec![0u8; out_stride * out_h as usize];
            let src_stride = (cur_w as usize) * 4;

            for oy in 0..out_h {
                for ox in 0..out_w {
                    // `inverse_point` returns source coordinate for this output pixel.
                    // Centre-of-pixel convention: the center of pixel (ox, oy) is (ox + 0.5, oy + 0.5).
                    let (sx, sy) = match effect
                        .effect
                        .inverse_point(ox as f32 + 0.5, oy as f32 + 0.5, in_w, in_h)
                    {
                        Some(p) => p,
                        None => {
                            // Content-adaptive effect — skip for now (fill with transparent).
                            continue;
                        }
                    };
                    let sx = sx - 0.5;
                    let sy = sy - 0.5;
                    let dst = &mut out[(oy as usize) * out_stride + (ox as usize) * 4..];

                    sample_bilinear_rgba8(&cur_data, src_stride, cur_w, cur_h, sx, sy, dst);
                }
            }

            cur_data = out;
            cur_w = out_w;
            cur_h = out_h;
        }

        Ok(Self {
            data: cur_data,
            width: cur_w,
            height: cur_h,
            format: fmt,
            strip_height: 16.min(cur_h),
            y: 0,
        })
    }
}

impl Source for EffectSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
        if self.y >= self.height {
            return Ok(None);
        }
        let rows = self.strip_height.min(self.height - self.y);
        let stride = self.format.aligned_stride(self.width);
        let start = self.y as usize * stride;
        let end = (self.y + rows) as usize * stride;
        self.y += rows;
        Ok(Some(
            Strip::new(&self.data[start..end], self.width, rows, stride, self.format).pipe_err()?,
        ))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        self.format
    }
}

/// Bilinear sample 4 neighbors with clamp-to-edge, write to `dst[0..4]`.
///
/// Out-of-bounds source coordinates produce transparent (0,0,0,0) to
/// match the expected "fill with transparent" behavior of RotateMode::Expand
/// without an explicit fill color. Callers that want a fill color should
/// pre-fill the output buffer.
#[inline]
fn sample_bilinear_rgba8(
    src: &[u8],
    stride: usize,
    src_w: u32,
    src_h: u32,
    sx: f32,
    sy: f32,
    dst: &mut [u8],
) {
    // If the source point is more than half a pixel outside the source grid,
    // treat as fully out-of-bounds (transparent).
    if sx < -0.5 || sy < -0.5 || sx > src_w as f32 - 0.5 || sy > src_h as f32 - 0.5 {
        dst[0] = 0;
        dst[1] = 0;
        dst[2] = 0;
        dst[3] = 0;
        return;
    }

    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;

    // Clamp each corner.
    let x0c = x0.clamp(0, src_w as i32 - 1) as u32;
    let y0c = y0.clamp(0, src_h as i32 - 1) as u32;
    let x1c = (x0 + 1).clamp(0, src_w as i32 - 1) as u32;
    let y1c = (y0 + 1).clamp(0, src_h as i32 - 1) as u32;

    let p00 = &src[y0c as usize * stride + x0c as usize * 4..][..4];
    let p10 = &src[y0c as usize * stride + x1c as usize * 4..][..4];
    let p01 = &src[y1c as usize * stride + x0c as usize * 4..][..4];
    let p11 = &src[y1c as usize * stride + x1c as usize * 4..][..4];

    let w00 = (1.0 - fx) * (1.0 - fy);
    let w10 = fx * (1.0 - fy);
    let w01 = (1.0 - fx) * fy;
    let w11 = fx * fy;

    for c in 0..4 {
        let v = p00[c] as f32 * w00 + p10[c] as f32 * w10 + p01[c] as f32 * w01 + p11[c] as f32 * w11;
        dst[c] = v.round().clamp(0.0, 255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::MaterializedSource;
    use zenlayout::dimension::{RotateEffect, RotateMode};
    use zenlayout::Size;

    fn solid_red_source(w: u32, h: u32) -> Box<dyn Source> {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for chunk in data.chunks_exact_mut(4) {
            chunk[0] = 255; // R
            chunk[3] = 255; // A
        }
        Box::new(MaterializedSource::from_data(
            data,
            w,
            h,
            format::RGBA8_SRGB,
        ))
    }

    /// 0° rotation should produce identity output.
    #[test]
    fn rotate_zero_degrees_is_identity() {
        let src = solid_red_source(10, 10);
        let effect = ResolvedEffect {
            effect: Box::new(RotateEffect::from_degrees(0.0, RotateMode::CropToOriginal)),
            input_dims: Size::new(10, 10),
            output_dims: Size::new(10, 10),
            command_index: 0,
            before_resize: true,
        };
        let mut es = EffectSource::new(src, &[effect], &Limits::default()).unwrap();
        assert_eq!(es.width(), 10);
        assert_eq!(es.height(), 10);

        // Every pixel should still be red.
        let mut all_rows_red = true;
        while let Some(strip) = es.next().unwrap() {
            for r in 0..strip.rows() {
                for x in 0..strip.width() {
                    let row = strip.row(r);
                    let px = &row[x as usize * 4..(x as usize + 1) * 4];
                    if px[0] != 255 || px[3] != 255 {
                        all_rows_red = false;
                    }
                }
            }
        }
        assert!(all_rows_red, "0° rotation should preserve red fill");
    }

    /// 5° rotation on CropToOriginal preserves dimensions.
    #[test]
    fn rotate_5_degrees_crop_to_original_preserves_dims() {
        let src = solid_red_source(100, 100);
        let effect = ResolvedEffect {
            effect: Box::new(RotateEffect::from_degrees(5.0, RotateMode::CropToOriginal)),
            input_dims: Size::new(100, 100),
            output_dims: Size::new(100, 100),
            command_index: 0,
            before_resize: true,
        };
        let es = EffectSource::new(src, &[effect], &Limits::default()).unwrap();
        assert_eq!(es.width(), 100);
        assert_eq!(es.height(), 100);
    }

    /// 5° rotation on InscribedCrop shrinks dimensions.
    #[test]
    fn rotate_5_degrees_inscribed_crop_shrinks() {
        let src = solid_red_source(100, 100);
        let angle_deg = 5.0f32;
        let effect_for_dims = RotateEffect::from_degrees(angle_deg, RotateMode::InscribedCrop);
        use zenlayout::dimension::DimensionEffect;
        let (ow, oh) = effect_for_dims.forward(100, 100).unwrap();
        assert!(ow < 100 && oh < 100, "inscribed crop must shrink");

        let effect = ResolvedEffect {
            effect: Box::new(effect_for_dims),
            input_dims: Size::new(100, 100),
            output_dims: Size::new(ow, oh),
            command_index: 0,
            before_resize: true,
        };
        let mut es = EffectSource::new(src, &[effect], &Limits::default()).unwrap();
        assert_eq!(es.width(), ow);
        assert_eq!(es.height(), oh);
        // All pixels of the inscribed crop should still be red (solid-color source).
        while let Some(strip) = es.next().unwrap() {
            for r in 0..strip.rows() {
                let row = strip.row(r);
                for x in 0..strip.width() {
                    let px = &row[x as usize * 4..(x as usize + 1) * 4];
                    assert_eq!(px[0], 255, "inscribed crop of red image should be red");
                }
            }
        }
    }

    /// 5° rotation on Expand grows dimensions.
    #[test]
    fn rotate_5_degrees_expand_grows() {
        let src = solid_red_source(100, 100);
        let angle_deg = 5.0f32;
        let effect_for_dims = RotateEffect::from_degrees(
            angle_deg,
            RotateMode::Expand {
                color: zenlayout::CanvasColor::Transparent,
            },
        );
        use zenlayout::dimension::DimensionEffect;
        let (ow, oh) = effect_for_dims.forward(100, 100).unwrap();
        assert!(ow > 100 && oh > 100, "expand must grow");

        let effect = ResolvedEffect {
            effect: Box::new(effect_for_dims),
            input_dims: Size::new(100, 100),
            output_dims: Size::new(ow, oh),
            command_index: 0,
            before_resize: true,
        };
        let es = EffectSource::new(src, &[effect], &Limits::default()).unwrap();
        assert_eq!(es.width(), ow);
        assert_eq!(es.height(), oh);
    }
}
