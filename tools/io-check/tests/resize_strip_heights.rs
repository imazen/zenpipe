// Copied verbatim from main's tests/resize_strip_heights.rs at dd63d9d13f, so the
// adopted fix is checked on this branch's graph (the zenpipe package's own test
// targets cannot build on zencodec 0.2 yet).
//! A resize must not depend on how tall its upstream strips are.
//!
//! The resizer's ring buffer is sized for a 16-row batch, but nothing in the
//! `Source` contract bounds upstream strip height: a decoder or a materialized
//! frame may hand over the whole image as one strip. Before the fix, a strip
//! taller than the ring failed with "ring buffer overflow" (seen through
//! `ImageJob` on a 48x40 PNG with `w=24`). Every strip height must succeed and
//! produce identical pixels.

use zenpipe::Source;
use zenpipe::format;
use zenpipe::sources::{MaterializedSource, ResizeF32Source, ResizeSource};

const W: u32 = 48;
const H: u32 = 64;

/// Packed output rows (strip stride padding removed).
fn drain(mut source: Box<dyn Source>) -> Vec<u8> {
    let row_len = source.width() as usize * source.format().bytes_per_pixel();
    let mut out = Vec::new();
    while let Some(strip) = source.next().expect("next") {
        for r in 0..strip.rows() {
            out.extend_from_slice(&strip.row(r)[..row_len]);
        }
    }
    out
}

fn rgba8_input(strip_height: u32) -> Box<dyn Source> {
    let fmt = format::RGBA8_SRGB;
    let stride = fmt.aligned_stride(W);
    let mut data = vec![0u8; stride * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let p = y * stride + x * 4;
            data[p] = (x * 5) as u8;
            data[p + 1] = (y * 3) as u8;
            data[p + 2] = ((x + y) * 7) as u8;
            data[p + 3] = 255 - (y * 2) as u8;
        }
    }
    Box::new(MaterializedSource::from_data(data, W, H, fmt).with_strip_height(strip_height))
}

fn f32_input(strip_height: u32) -> Box<dyn Source> {
    let fmt = format::RGBAF32_LINEAR;
    let stride = fmt.aligned_stride(W);
    let mut data = vec![0u8; stride * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let px = [
                x as f32 / W as f32,
                y as f32 / H as f32,
                ((x + y) % 7) as f32 / 7.0,
                1.0,
            ];
            for (c, v) in px.iter().enumerate() {
                let p = y * stride + x * 16 + c * 4;
                data[p..p + 4].copy_from_slice(&v.to_ne_bytes());
            }
        }
    }
    Box::new(MaterializedSource::from_data(data, W, H, fmt).with_strip_height(strip_height))
}

/// Downscale and upscale, so both a wide input window per output row and
/// several output rows per input row are covered.
const TARGETS: [(u32, u32); 2] = [(12, 10), (96, 128)];
const STRIP_HEIGHTS: [u32; 5] = [1, 7, 16, 40, H];

#[test]
fn rgba8_resize_is_independent_of_upstream_strip_height() {
    for (ow, oh) in TARGETS {
        let config = zenresize::ResizeConfig::builder(W, H, ow, oh).build();
        let reference = drain(Box::new(
            ResizeSource::new(rgba8_input(16), &config, 16).expect("source"),
        ));
        assert_eq!(reference.len(), (ow * oh * 4) as usize);
        for sh in STRIP_HEIGHTS {
            let out = drain(Box::new(
                ResizeSource::new(rgba8_input(sh), &config, 16).expect("source"),
            ));
            assert!(
                out == reference,
                "{W}x{H} -> {ow}x{oh}, upstream strips of {sh} rows"
            );
        }
    }
}

#[test]
fn f32_resize_is_independent_of_upstream_strip_height() {
    for (ow, oh) in TARGETS {
        let config = zenresize::ResizeConfig::builder(W, H, ow, oh)
            .format(zenresize::PixelDescriptor::RGBAF32_LINEAR)
            .build();
        let reference = drain(Box::new(
            ResizeF32Source::new(f32_input(16), &config, 16).expect("source"),
        ));
        assert_eq!(reference.len(), (ow * oh * 16) as usize);
        for sh in STRIP_HEIGHTS {
            let out = drain(Box::new(
                ResizeF32Source::new(f32_input(sh), &config, 16).expect("source"),
            ));
            assert!(
                out == reference,
                "{W}x{H} -> {ow}x{oh}, upstream strips of {sh} rows"
            );
        }
    }
}
