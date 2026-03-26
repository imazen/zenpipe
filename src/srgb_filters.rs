//! Legacy sRGB u8 filter implementations for backwards compatibility.
//!
//! These filters operate directly on sRGB gamma-encoded u8 pixel data,
//! replicating imageflow4's exact behavior. They exist **only** for
//! backwards compatibility — new code should use the Oklab pipeline
//! which produces perceptually correct results.
//!
//! All operations in this module are perceptually incorrect because
//! they apply arithmetic in gamma space rather than linear light or
//! a perceptual color space. Known artifacts:
//! - Contrast shifts hue and saturation
//! - Saturation uses BT.709 luma in gamma space (wrong weights)
//! - Sharpen produces color fringing at edges
//! - Blur darkens color boundaries (gamma-space averaging)
//!
//! Per-pixel operations ([`color_adjust`], [`color_matrix`], [`color_filter`])
//! accept `PixelSliceMut` and process row-by-row for streaming compatibility.
//! Neighborhood operations ([`sharpen`], [`blur`]) require the full image
//! and accept `PixelSlice` + output `PixelBuffer`.
//!
//! Requires the `srgb-filters` feature.

use alloc::vec;
use alloc::vec::Vec;
use rgb::Rgba;
use zenpixels::buffer::{PixelBuffer, PixelSliceMut};
use zenpixels::{PixelDescriptor, TransferFunction};
use zenpixels_convert::RowConverter;

/// sRGB gamma-space color adjustments (streaming, row-by-row).
///
/// Applies brightness, contrast, and saturation directly to sRGB u8 values.
/// Processes the slice in-place by converting each row to RGBA8, applying
/// the adjustments, and converting back.
///
/// This produces the same results as imageflow4's `ColorAdjust` step.
pub fn color_adjust(
    slice: &mut PixelSliceMut<'_>,
    brightness: f32,
    contrast: f32,
    saturation: f32,
) {
    if brightness == 0.0 && contrast == 0.0 && saturation == 0.0 {
        return;
    }
    with_rows_rgba(slice, |row| {
        for p in row.iter_mut() {
            let mut r = p.r as f32 / 255.0;
            let mut g = p.g as f32 / 255.0;
            let mut b = p.b as f32 / 255.0;

            r += brightness;
            g += brightness;
            b += brightness;

            r = (r - 0.5) * (1.0 + contrast) + 0.5;
            g = (g - 0.5) * (1.0 + contrast) + 0.5;
            b = (b - 0.5) * (1.0 + contrast) + 0.5;

            if saturation != 0.0 {
                let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let s = 1.0 + saturation;
                r = lum + (r - lum) * s;
                g = lum + (g - lum) * s;
                b = lum + (b - lum) * s;
            }

            p.r = (r * 255.0).max(0.0).min(255.0) as u8;
            p.g = (g * 255.0).max(0.0).min(255.0) as u8;
            p.b = (b * 255.0).max(0.0).min(255.0) as u8;
        }
    });
}

/// 5×5 color matrix in sRGB gamma space (streaming, row-by-row).
///
/// Row-major: `[R',G',B',A',1] = M × [R,G,B,A,1]` where R,G,B,A are
/// normalized sRGB values in `[0, 1]`. This is imageflow4's `ColorMatrix`.
pub fn color_matrix(slice: &mut PixelSliceMut<'_>, matrix: &[f32; 25]) {
    let m = *matrix;
    with_rows_rgba(slice, |row| {
        for p in row.iter_mut() {
            let r = p.r as f32 / 255.0;
            let g = p.g as f32 / 255.0;
            let b = p.b as f32 / 255.0;
            let a = p.a as f32 / 255.0;

            let nr = m[0] * r + m[1] * g + m[2] * b + m[3] * a + m[4];
            let ng = m[5] * r + m[6] * g + m[7] * b + m[8] * a + m[9];
            let nb = m[10] * r + m[11] * g + m[12] * b + m[13] * a + m[14];
            let na = m[15] * r + m[16] * g + m[17] * b + m[18] * a + m[19];

            p.r = (nr * 255.0).max(0.0).min(255.0) as u8;
            p.g = (ng * 255.0).max(0.0).min(255.0) as u8;
            p.b = (nb * 255.0).max(0.0).min(255.0) as u8;
            p.a = (na * 255.0).max(0.0).min(255.0) as u8;
        }
    });
}

/// Predefined color filter operations in sRGB space.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SrgbColorFilter {
    /// BT.709 luma grayscale (0.2126 R + 0.7152 G + 0.0722 B).
    GrayscaleBt709,
    /// NTSC luma grayscale (0.299 R + 0.587 G + 0.114 B).
    GrayscaleNtsc,
    /// Flat average grayscale ((R + G + B) / 3).
    GrayscaleFlat,
    /// Classic sepia tone matrix.
    Sepia,
    /// Invert RGB channels (255 - v), preserve alpha.
    Invert,
    /// Scale alpha by a factor (0.0 = transparent, 1.0 = opaque).
    Alpha(f32),
}

/// Apply a predefined color filter in sRGB space (streaming, row-by-row).
///
/// Replicates imageflow4's `ColorFilter` step exactly.
pub fn color_filter(slice: &mut PixelSliceMut<'_>, filter: &SrgbColorFilter) {
    match filter {
        SrgbColorFilter::GrayscaleBt709 => with_rows_rgba(slice, |row| {
            for p in row.iter_mut() {
                let lum = (0.2126 * p.r as f32 + 0.7152 * p.g as f32 + 0.0722 * p.b as f32) as u8;
                p.r = lum;
                p.g = lum;
                p.b = lum;
            }
        }),
        SrgbColorFilter::GrayscaleNtsc => with_rows_rgba(slice, |row| {
            for p in row.iter_mut() {
                let lum = (0.299 * p.r as f32 + 0.587 * p.g as f32 + 0.114 * p.b as f32) as u8;
                p.r = lum;
                p.g = lum;
                p.b = lum;
            }
        }),
        SrgbColorFilter::GrayscaleFlat => with_rows_rgba(slice, |row| {
            for p in row.iter_mut() {
                let lum = ((p.r as u16 + p.g as u16 + p.b as u16) / 3) as u8;
                p.r = lum;
                p.g = lum;
                p.b = lum;
            }
        }),
        SrgbColorFilter::Sepia => with_rows_rgba(slice, |row| {
            for p in row.iter_mut() {
                let r = p.r as f32;
                let g = p.g as f32;
                let b = p.b as f32;
                p.r = (0.393 * r + 0.769 * g + 0.189 * b).min(255.0) as u8;
                p.g = (0.349 * r + 0.686 * g + 0.168 * b).min(255.0) as u8;
                p.b = (0.272 * r + 0.534 * g + 0.131 * b).min(255.0) as u8;
            }
        }),
        SrgbColorFilter::Invert => with_rows_rgba(slice, |row| {
            for p in row.iter_mut() {
                p.r = 255 - p.r;
                p.g = 255 - p.g;
                p.b = 255 - p.b;
            }
        }),
        SrgbColorFilter::Alpha(a) => {
            let alpha_mul = (*a * 255.0) as u16;
            with_rows_rgba(slice, |row| {
                for p in row.iter_mut() {
                    p.a = ((p.a as u16 * alpha_mul) / 255) as u8;
                }
            });
        }
    }
}

/// Unsharp mask sharpening in sRGB space.
///
/// Replicates imageflow4's `Sharpen` step: box-blur with radius 2,
/// then unsharp mask on all RGB channels (not alpha). Amount is 0-100
/// (same scale as imageflow4).
///
/// This is a neighborhood operation — it needs the full image. Takes
/// a `PixelBuffer` and modifies in-place.
pub fn sharpen(pixels: &mut PixelBuffer, amount: f32) {
    let amount = amount / 100.0;
    let width = pixels.width();
    let height = pixels.height();
    let w = width as usize;
    let h = height as usize;

    let packed = collect_rgba(pixels);
    let blurred = box_blur_rgba(&packed, w, h, 2);

    let mut out = packed.clone();
    for i in 0..out.len() {
        let o = packed[i];
        let b = blurred[i];
        out[i] = Rgba {
            r: ((o.r as f32 + amount * (o.r as f32 - b.r as f32))
                .max(0.0)
                .min(255.0)) as u8,
            g: ((o.g as f32 + amount * (o.g as f32 - b.g as f32))
                .max(0.0)
                .min(255.0)) as u8,
            b: ((o.b as f32 + amount * (o.b as f32 - b.b as f32))
                .max(0.0)
                .min(255.0)) as u8,
            a: o.a,
        };
    }

    *pixels = PixelBuffer::from_pixels_erased(out, width, height).unwrap();
}

/// Gaussian-approximation blur in sRGB space (3× box blur).
///
/// Replicates imageflow4's `Blur` step exactly. Neighborhood operation
/// that modifies a `PixelBuffer` in-place.
pub fn blur(pixels: &mut PixelBuffer, sigma: f32) {
    let radius = (sigma * 2.0).ceil() as usize;
    if radius == 0 {
        return;
    }
    let width = pixels.width();
    let height = pixels.height();
    let w = width as usize;
    let h = height as usize;

    let mut packed = collect_rgba(pixels);

    let pass_radius = (radius / 3).max(1);
    for _ in 0..3 {
        packed = box_blur_rgba(&packed, w, h, pass_radius);
    }

    *pixels = PixelBuffer::from_pixels_erased(packed, width, height).unwrap();
}

// ─── Internal helpers ─────────────────────────────────────────────────

/// Collect all pixels from a PixelBuffer as packed RGBA8, converting if needed.
fn collect_rgba(pixels: &PixelBuffer) -> Vec<Rgba<u8>> {
    let desc = pixels.descriptor();
    let width = pixels.width();
    let height = pixels.height();
    let w = width as usize;
    let h = height as usize;
    let slice = pixels.as_slice();

    let rgba_desc = PixelDescriptor::RGBA8_SRGB;
    let need_convert = desc != rgba_desc;
    let mut converter = need_convert
        .then(|| RowConverter::new(desc, rgba_desc).expect("conversion to RGBA8 should succeed"));

    let mut packed = Vec::with_capacity(w * h);
    let mut row_buf = vec![0u8; w * 4];

    for y in 0..h as u32 {
        let src_row = slice.row(y);
        let rgba_row = if let Some(conv) = &mut converter {
            conv.convert_row(src_row, &mut row_buf, width);
            &row_buf[..w * 4]
        } else {
            &src_row[..w * 4]
        };
        let pixels_row: &[Rgba<u8>] = bytemuck::cast_slice(rgba_row);
        packed.extend_from_slice(pixels_row);
    }
    packed
}

/// Process rows of a PixelSliceMut as RGBA8 pixels, converting if needed.
///
/// If the slice is already RGBA8 sRGB, operates in-place without conversion.
/// Otherwise, converts each row to RGBA8, applies the function, and converts back.
fn with_rows_rgba(slice: &mut PixelSliceMut<'_>, mut f: impl FnMut(&mut [Rgba<u8>])) {
    let desc = slice.descriptor();
    let width = slice.width();
    let height = slice.rows();
    let w = width as usize;

    let rgba_desc = PixelDescriptor::new(
        zenpixels::ChannelType::U8,
        zenpixels::ChannelLayout::Rgba,
        Some(zenpixels::AlphaMode::Straight),
        desc.transfer(),
    );

    let is_rgba8 = desc.channel_type() == zenpixels::ChannelType::U8
        && desc.layout() == zenpixels::ChannelLayout::Rgba
        && desc.transfer() != TransferFunction::Unknown;

    if is_rgba8 {
        // Fast path: operate directly on the row bytes
        for y in 0..height {
            let row_bytes = slice.row_mut(y);
            let pixels: &mut [Rgba<u8>] = bytemuck::cast_slice_mut(&mut row_bytes[..w * 4]);
            f(pixels);
        }
    } else {
        // Convert each row to RGBA8, process, convert back
        let mut to_rgba =
            RowConverter::new(desc, rgba_desc).expect("conversion to RGBA8 should succeed");
        let mut from_rgba =
            RowConverter::new(rgba_desc, desc).expect("conversion from RGBA8 should succeed");

        let mut rgba_buf = vec![0u8; w * 4];
        let mut back_buf = vec![0u8; w * desc.bytes_per_pixel()];

        for y in 0..height {
            // Convert to RGBA8
            let row_bytes = slice.row_mut(y);
            to_rgba.convert_row(row_bytes, &mut rgba_buf, width);

            // Apply function
            let pixels: &mut [Rgba<u8>] = bytemuck::cast_slice_mut(&mut rgba_buf[..w * 4]);
            f(pixels);

            // Convert back
            from_rgba.convert_row(&rgba_buf, &mut back_buf, width);
            let row_bytes = slice.row_mut(y);
            row_bytes[..back_buf.len()].copy_from_slice(&back_buf);
        }
    }
}

/// Separable box blur on tightly-packed RGBA8 buffer.
///
/// Exact replica of imageflow4's `box_blur_rgba`.
fn box_blur_rgba(input: &[Rgba<u8>], w: usize, h: usize, radius: usize) -> Vec<Rgba<u8>> {
    let diameter = 2 * radius + 1;
    let inv = 1.0 / diameter as f32;

    // Horizontal pass
    let mut temp = vec![
        Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0
        };
        w * h
    ];
    for y in 0..h {
        for x in 0..w {
            let (mut rs, mut gs, mut bs, mut a_s) = (0u32, 0u32, 0u32, 0u32);
            for di in 0..diameter {
                let sx = (x as i64 + di as i64 - radius as i64)
                    .max(0)
                    .min(w as i64 - 1) as usize;
                let p = input[y * w + sx];
                rs += p.r as u32;
                gs += p.g as u32;
                bs += p.b as u32;
                a_s += p.a as u32;
            }
            temp[y * w + x] = Rgba {
                r: (rs as f32 * inv) as u8,
                g: (gs as f32 * inv) as u8,
                b: (bs as f32 * inv) as u8,
                a: (a_s as f32 * inv) as u8,
            };
        }
    }

    // Vertical pass
    let mut output = vec![
        Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0
        };
        w * h
    ];
    for x in 0..w {
        for y in 0..h {
            let (mut rs, mut gs, mut bs, mut a_s) = (0u32, 0u32, 0u32, 0u32);
            for di in 0..diameter {
                let sy = (y as i64 + di as i64 - radius as i64)
                    .max(0)
                    .min(h as i64 - 1) as usize;
                let p = temp[sy * w + x];
                rs += p.r as u32;
                gs += p.g as u32;
                bs += p.b as u32;
                a_s += p.a as u32;
            }
            output[y * w + x] = Rgba {
                r: (rs as f32 * inv) as u8,
                g: (gs as f32 * inv) as u8,
                b: (bs as f32 * inv) as u8,
                a: (a_s as f32 * inv) as u8,
            };
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_buffer(width: u32, height: u32) -> PixelBuffer {
        let n = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(n * 4);
        for i in 0..n {
            let t = i as f32 / n as f32;
            data.push((t * 200.0 + 30.0) as u8);
            data.push(((1.0 - t) * 180.0 + 40.0) as u8);
            data.push((t * 100.0 + 80.0) as u8);
            data.push(255u8);
        }
        PixelBuffer::from_vec(data, width, height, PixelDescriptor::RGBA8_SRGB).unwrap()
    }

    fn make_step_edge_buffer(width: u32, height: u32) -> PixelBuffer {
        let n = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(n * 4);
        for i in 0..n {
            let x = i % width as usize;
            let v = if x < width as usize / 2 { 50u8 } else { 200u8 };
            data.push(v);
            data.push(v);
            data.push(v);
            data.push(255u8);
        }
        PixelBuffer::from_vec(data, width, height, PixelDescriptor::RGBA8_SRGB).unwrap()
    }

    fn avg_brightness(buf: &PixelBuffer) -> f32 {
        let bytes = buf.copy_to_contiguous_bytes();
        let n = bytes.len() / 4;
        let mut sum = 0u64;
        for i in 0..n {
            sum += bytes[i * 4] as u64;
            sum += bytes[i * 4 + 1] as u64;
            sum += bytes[i * 4 + 2] as u64;
        }
        sum as f32 / (n * 3) as f32
    }

    // ─── ColorAdjust ──────────────────────────────────────────────────

    #[test]
    fn color_adjust_zero_is_identity() {
        let mut buf = make_test_buffer(16, 16);
        let orig = buf.copy_to_contiguous_bytes();
        color_adjust(&mut buf.as_slice_mut(), 0.0, 0.0, 0.0);
        let after = buf.copy_to_contiguous_bytes();
        assert_eq!(orig, after);
    }

    #[test]
    fn color_adjust_brightness_increases() {
        let mut buf = make_test_buffer(16, 16);
        let before = avg_brightness(&buf);
        color_adjust(&mut buf.as_slice_mut(), 0.2, 0.0, 0.0);
        let after = avg_brightness(&buf);
        assert!(
            after > before,
            "brightness +0.2 should increase avg: {before} -> {after}"
        );
    }

    #[test]
    fn color_adjust_contrast_increases_range() {
        let mut buf = make_test_buffer(32, 32);
        color_adjust(&mut buf.as_slice_mut(), 0.0, 0.5, 0.0);
        let bytes = buf.copy_to_contiguous_bytes();
        let n = bytes.len() / 4;
        let mut has_bright = false;
        let mut has_dark = false;
        for i in 0..n {
            if bytes[i * 4] > 200 {
                has_bright = true;
            }
            if bytes[i * 4] < 50 {
                has_dark = true;
            }
        }
        assert!(has_bright || has_dark, "contrast should increase range");
    }

    // ─── ColorMatrix ──────────────────────────────────────────────────

    #[test]
    fn color_matrix_identity() {
        let mut buf = make_test_buffer(8, 8);
        let orig = buf.copy_to_contiguous_bytes();
        #[rustfmt::skip]
        let identity: [f32; 25] = [
            1.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        color_matrix(&mut buf.as_slice_mut(), &identity);
        let after = buf.copy_to_contiguous_bytes();
        assert_eq!(orig, after);
    }

    #[test]
    fn color_matrix_sepia() {
        let mut buf = make_test_buffer(8, 8);
        #[rustfmt::skip]
        let sepia: [f32; 25] = [
            0.393, 0.769, 0.189, 0.0, 0.0,
            0.349, 0.686, 0.168, 0.0, 0.0,
            0.272, 0.534, 0.131, 0.0, 0.0,
            0.0,   0.0,   0.0,   1.0, 0.0,
            0.0,   0.0,   0.0,   0.0, 1.0,
        ];
        color_matrix(&mut buf.as_slice_mut(), &sepia);
        let bytes = buf.copy_to_contiguous_bytes();
        assert!(bytes[0] >= bytes[1], "sepia: R should >= G");
        assert!(bytes[1] >= bytes[2], "sepia: G should >= B");
    }

    // ─── ColorFilter ──────────────────────────────────────────────────

    #[test]
    fn grayscale_bt709_produces_neutral() {
        let mut buf = make_test_buffer(8, 8);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::GrayscaleBt709);
        let bytes = buf.copy_to_contiguous_bytes();
        for i in 0..bytes.len() / 4 {
            assert_eq!(bytes[i * 4], bytes[i * 4 + 1], "R == G for grayscale");
            assert_eq!(bytes[i * 4 + 1], bytes[i * 4 + 2], "G == B for grayscale");
        }
    }

    #[test]
    fn grayscale_ntsc_produces_neutral() {
        let mut buf = make_test_buffer(8, 8);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::GrayscaleNtsc);
        let bytes = buf.copy_to_contiguous_bytes();
        for i in 0..bytes.len() / 4 {
            assert_eq!(bytes[i * 4], bytes[i * 4 + 1]);
            assert_eq!(bytes[i * 4 + 1], bytes[i * 4 + 2]);
        }
    }

    #[test]
    fn grayscale_flat_produces_neutral() {
        let mut buf = make_test_buffer(8, 8);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::GrayscaleFlat);
        let bytes = buf.copy_to_contiguous_bytes();
        for i in 0..bytes.len() / 4 {
            assert_eq!(bytes[i * 4], bytes[i * 4 + 1]);
            assert_eq!(bytes[i * 4 + 1], bytes[i * 4 + 2]);
        }
    }

    #[test]
    fn invert_double_is_identity() {
        let mut buf = make_test_buffer(8, 8);
        let orig = buf.copy_to_contiguous_bytes();
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::Invert);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::Invert);
        let after = buf.copy_to_contiguous_bytes();
        assert_eq!(orig, after);
    }

    #[test]
    fn alpha_zero_is_transparent() {
        let mut buf = make_test_buffer(8, 8);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::Alpha(0.0));
        let bytes = buf.copy_to_contiguous_bytes();
        for i in 0..bytes.len() / 4 {
            assert_eq!(bytes[i * 4 + 3], 0, "alpha should be 0");
        }
    }

    #[test]
    fn alpha_one_preserves() {
        let mut buf = make_test_buffer(8, 8);
        let orig = buf.copy_to_contiguous_bytes();
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::Alpha(1.0));
        let after = buf.copy_to_contiguous_bytes();
        assert_eq!(orig, after);
    }

    #[test]
    fn sepia_has_warm_tint() {
        let mut buf = make_test_buffer(8, 8);
        color_filter(&mut buf.as_slice_mut(), &SrgbColorFilter::Sepia);
        let bytes = buf.copy_to_contiguous_bytes();
        let mut warm = false;
        for i in 0..bytes.len() / 4 {
            if bytes[i * 4] >= bytes[i * 4 + 1] && bytes[i * 4 + 1] >= bytes[i * 4 + 2] {
                warm = true;
                break;
            }
        }
        assert!(warm, "sepia should produce warm tint (R >= G >= B)");
    }

    // ─── Sharpen ──────────────────────────────────────────────────────

    #[test]
    fn sharpen_increases_edge_contrast() {
        let mut buf = make_step_edge_buffer(32, 32);
        sharpen(&mut buf, 50.0);
        let bytes = buf.copy_to_contiguous_bytes();
        let w = 32usize;
        let mid_row = 16 * w;
        let left = bytes[(mid_row + 14) * 4];
        let right = bytes[(mid_row + 17) * 4];
        assert!(
            right as i32 - left as i32 > 150,
            "sharpen should increase edge contrast: left={left}, right={right}"
        );
    }

    // ─── Blur ─────────────────────────────────────────────────────────

    #[test]
    fn blur_zero_sigma_is_noop() {
        let mut buf = make_test_buffer(8, 8);
        let orig = buf.copy_to_contiguous_bytes();
        blur(&mut buf, 0.0);
        let after = buf.copy_to_contiguous_bytes();
        assert_eq!(orig, after);
    }

    #[test]
    fn blur_reduces_edge_contrast() {
        let mut buf = make_step_edge_buffer(32, 32);
        let orig_bytes = buf.copy_to_contiguous_bytes();
        let w = 32usize;
        let mid_row = 16 * w;
        let orig_left = orig_bytes[(mid_row + 14) * 4];
        let orig_right = orig_bytes[(mid_row + 17) * 4];
        let orig_diff = orig_right as i32 - orig_left as i32;

        blur(&mut buf, 3.0);
        let bytes = buf.copy_to_contiguous_bytes();
        let left = bytes[(mid_row + 14) * 4];
        let right = bytes[(mid_row + 17) * 4];
        let blurred_diff = right as i32 - left as i32;

        assert!(
            blurred_diff < orig_diff,
            "blur should reduce edge contrast: orig={orig_diff}, blurred={blurred_diff}"
        );
    }

    #[test]
    fn blur_constant_stays_constant() {
        let n = 16 * 16;
        let data: Vec<u8> = (0..n).flat_map(|_| [128u8, 128, 128, 255]).collect();
        let mut buf = PixelBuffer::from_vec(data, 16, 16, PixelDescriptor::RGBA8_SRGB).unwrap();
        blur(&mut buf, 2.0);
        let bytes = buf.copy_to_contiguous_bytes();
        for i in 0..n {
            assert!(
                (bytes[i * 4] as i32 - 128).abs() <= 1,
                "constant should stay constant"
            );
        }
    }
}
