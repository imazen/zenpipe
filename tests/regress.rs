//! Regression tests: encode -> decode -> RGBA8 checksum for every codec x pixel format.
//!
//! Each test encodes a deterministic gradient image through a specific pixel format,
//! decodes it back, converts to RGBA8, and checks the hash against stored baselines.
//!
//! First run: `UPDATE_CHECKSUMS=1 cargo test --features all --test regress`
//! Verify:    `cargo test --features all --test regress`

use std::path::Path;

use imgref::ImgVec;
use zencodecs::pixel::{Bgra, Gray, Rgb, Rgba};
use zencodecs::{
    DecodeRequest, EncodeOutput, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _,
};
use zensim_regress::checksums::ChecksumManager;
use zensim_regress::generators;

const W: u32 = 128;
const H: u32 = 128;

fn manager() -> ChecksumManager {
    ChecksumManager::new(Path::new("tests/checksums"))
}

// ---------------------------------------------------------------------------
// sRGB u8 -> linear f32 helper (standard sRGB EOTF)
// ---------------------------------------------------------------------------

fn srgb_to_linear(v: u8) -> f32 {
    let s = v as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

// ---------------------------------------------------------------------------
// Gradient generators — all derived from the same RGBA8 source
// ---------------------------------------------------------------------------

fn gradient_rgba() -> Vec<u8> {
    generators::gradient(W, H)
}

fn gradient_rgb8() -> ImgVec<Rgb<u8>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Rgb<u8>> = rgba
        .chunks_exact(4)
        .map(|px| Rgb {
            r: px[0],
            g: px[1],
            b: px[2],
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_rgba8() -> ImgVec<Rgba<u8>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Rgba<u8>> = rgba
        .chunks_exact(4)
        .map(|px| Rgba {
            r: px[0],
            g: px[1],
            b: px[2],
            a: px[3],
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_bgra8() -> ImgVec<Bgra<u8>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Bgra<u8>> = rgba
        .chunks_exact(4)
        .map(|px| Bgra {
            b: px[2],
            g: px[1],
            r: px[0],
            a: px[3],
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_gray8() -> ImgVec<Gray<u8>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Gray<u8>> = rgba
        .chunks_exact(4)
        .map(|px| {
            // BT.601 luminance
            let y = (px[0] as u16 * 77 + px[1] as u16 * 150 + px[2] as u16 * 29) >> 8;
            Gray::new(y as u8)
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_rgb_f32() -> ImgVec<Rgb<f32>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Rgb<f32>> = rgba
        .chunks_exact(4)
        .map(|px| Rgb {
            r: srgb_to_linear(px[0]),
            g: srgb_to_linear(px[1]),
            b: srgb_to_linear(px[2]),
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_rgba_f32() -> ImgVec<Rgba<f32>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Rgba<f32>> = rgba
        .chunks_exact(4)
        .map(|px| Rgba {
            r: srgb_to_linear(px[0]),
            g: srgb_to_linear(px[1]),
            b: srgb_to_linear(px[2]),
            a: px[3] as f32 / 255.0,
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

fn gradient_gray_f32() -> ImgVec<Gray<f32>> {
    let rgba = gradient_rgba();
    let pixels: Vec<Gray<f32>> = rgba
        .chunks_exact(4)
        .map(|px| {
            let r = srgb_to_linear(px[0]);
            let g = srgb_to_linear(px[1]);
            let b = srgb_to_linear(px[2]);
            // BT.709 luminance in linear light
            Gray::new(0.2126 * r + 0.7152 * g + 0.0722 * b)
        })
        .collect();
    ImgVec::new(pixels, W as usize, H as usize)
}

// ---------------------------------------------------------------------------
// Core roundtrip check
// ---------------------------------------------------------------------------

/// Encode, decode, convert to RGBA8, and verify checksum against stored baseline.
fn check_roundtrip(
    encoded: Result<EncodeOutput, whereat::At<zencodecs::CodecError>>,
    codec_name: &str,
    format_name: &str,
) {
    let mgr = manager();
    let encoded =
        encoded.unwrap_or_else(|e| panic!("{codec_name}/{format_name} encode failed: {e}"));

    let decoded = DecodeRequest::new(encoded.as_ref())
        .decode_full_frame()
        .unwrap_or_else(|e| panic!("{codec_name}/{format_name} decode failed: {e}"));

    let rgba_buf = decoded.into_buffer().to_rgba8();
    let w = rgba_buf.width();
    let h = rgba_buf.height();
    let rgba_bytes = rgba_buf.copy_to_contiguous_bytes();

    let result = mgr
        .check_pixels(
            codec_name,
            "roundtrip",
            format_name,
            &rgba_bytes,
            w,
            h,
            None,
        )
        .unwrap_or_else(|e| panic!("{codec_name}/{format_name} check_pixels failed: {e}"));

    assert!(
        result.passed(),
        "{codec_name}/{format_name} regression check failed: {result}"
    );
}

// ---------------------------------------------------------------------------
// Test macro — generates one #[test] fn per codec x format combination
// ---------------------------------------------------------------------------

macro_rules! regress {
    // rgb8
    ($name:ident, $format:expr, $codec:literal, rgb8, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_rgb8();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_rgb8(img.as_ref()),
                $codec, "rgb8",
            );
        }
    };
    // rgba8
    ($name:ident, $format:expr, $codec:literal, rgba8, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_rgba8();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_rgba8(img.as_ref()),
                $codec, "rgba8",
            );
        }
    };
    // bgra8
    ($name:ident, $format:expr, $codec:literal, bgra8, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_bgra8();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_bgra8(img.as_ref()),
                $codec, "bgra8",
            );
        }
    };
    // bgrx8
    ($name:ident, $format:expr, $codec:literal, bgrx8, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_bgra8(); // bgrx uses same Bgra type, alpha ignored
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_bgrx8(img.as_ref()),
                $codec, "bgrx8",
            );
        }
    };
    // gray8
    ($name:ident, $format:expr, $codec:literal, gray8, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_gray8();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_gray8(img.as_ref()),
                $codec, "gray8",
            );
        }
    };
    // rgb_f32
    ($name:ident, $format:expr, $codec:literal, rgb_f32, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_rgb_f32();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_rgb_f32(img.as_ref()),
                $codec, "rgb_f32",
            );
        }
    };
    // rgba_f32
    ($name:ident, $format:expr, $codec:literal, rgba_f32, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_rgba_f32();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_rgba_f32(img.as_ref()),
                $codec, "rgba_f32",
            );
        }
    };
    // gray_f32
    ($name:ident, $format:expr, $codec:literal, gray_f32, $($feat:literal),+) => {
        #[test]
        $(#[cfg(feature = $feat)])+
        fn $name() {
            let img = gradient_gray_f32();
            check_roundtrip(
                EncodeRequest::new($format).with_quality(75.0).encode_full_frame_gray_f32(img.as_ref()),
                $codec, "gray_f32",
            );
        }
    };
}

// ===========================================================================
// JPEG — lossy, rgb8/rgba8/gray8/bgra8/rgb_f32
// ===========================================================================

regress!(regress_jpeg_rgb8, ImageFormat::Jpeg, "jpeg", rgb8, "jpeg");
regress!(regress_jpeg_rgba8, ImageFormat::Jpeg, "jpeg", rgba8, "jpeg");
regress!(regress_jpeg_gray8, ImageFormat::Jpeg, "jpeg", gray8, "jpeg");
regress!(regress_jpeg_bgra8, ImageFormat::Jpeg, "jpeg", bgra8, "jpeg");
regress!(
    regress_jpeg_rgb_f32,
    ImageFormat::Jpeg,
    "jpeg",
    rgb_f32,
    "jpeg"
);

// ===========================================================================
// WebP — lossy, rgb8/rgba8/bgra8/gray8/rgb_f32
// ===========================================================================

regress!(regress_webp_rgb8, ImageFormat::WebP, "webp", rgb8, "webp");
regress!(regress_webp_rgba8, ImageFormat::WebP, "webp", rgba8, "webp");
regress!(regress_webp_bgra8, ImageFormat::WebP, "webp", bgra8, "webp");
regress!(regress_webp_gray8, ImageFormat::WebP, "webp", gray8, "webp");
regress!(
    regress_webp_rgb_f32,
    ImageFormat::WebP,
    "webp",
    rgb_f32,
    "webp"
);

// ===========================================================================
// GIF — palette-quantized, rgba8/rgb8
// ===========================================================================

regress!(regress_gif_rgba8, ImageFormat::Gif, "gif", rgba8, "gif");
regress!(regress_gif_rgb8, ImageFormat::Gif, "gif", rgb8, "gif");

// ===========================================================================
// PNG — lossless, rgb8/rgba8/gray8/rgb_f32/rgba_f32/gray_f32
// ===========================================================================

regress!(regress_png_rgb8, ImageFormat::Png, "png", rgb8, "png");
regress!(regress_png_rgba8, ImageFormat::Png, "png", rgba8, "png");
regress!(regress_png_gray8, ImageFormat::Png, "png", gray8, "png");
regress!(regress_png_rgb_f32, ImageFormat::Png, "png", rgb_f32, "png");
regress!(
    regress_png_rgba_f32,
    ImageFormat::Png,
    "png",
    rgba_f32,
    "png"
);
regress!(
    regress_png_gray_f32,
    ImageFormat::Png,
    "png",
    gray_f32,
    "png"
);

// ===========================================================================
// AVIF — lossy (default), rgb8/rgba8/rgb_f32/rgba_f32/gray8/bgra8
// ===========================================================================

regress!(
    regress_avif_rgb8,
    ImageFormat::Avif,
    "avif",
    rgb8,
    "avif-encode",
    "avif-decode"
);
regress!(
    regress_avif_rgba8,
    ImageFormat::Avif,
    "avif",
    rgba8,
    "avif-encode",
    "avif-decode"
);
regress!(
    regress_avif_rgb_f32,
    ImageFormat::Avif,
    "avif",
    rgb_f32,
    "avif-encode",
    "avif-decode"
);
regress!(
    regress_avif_rgba_f32,
    ImageFormat::Avif,
    "avif",
    rgba_f32,
    "avif-encode",
    "avif-decode"
);
regress!(
    regress_avif_gray8,
    ImageFormat::Avif,
    "avif",
    gray8,
    "avif-encode",
    "avif-decode"
);
regress!(
    regress_avif_bgra8,
    ImageFormat::Avif,
    "avif",
    bgra8,
    "avif-encode",
    "avif-decode"
);

// ===========================================================================
// JXL — lossy (default), rgb8/rgba8/gray8/bgra8/bgrx8/rgb_f32/rgba_f32/gray_f32
// ===========================================================================

regress!(
    regress_jxl_rgb8,
    ImageFormat::Jxl,
    "jxl",
    rgb8,
    "jxl-encode",
    "jxl-decode"
);
regress!(
    regress_jxl_gray8,
    ImageFormat::Jxl,
    "jxl",
    gray8,
    "jxl-encode",
    "jxl-decode"
);
regress!(
    regress_jxl_rgb_f32,
    ImageFormat::Jxl,
    "jxl",
    rgb_f32,
    "jxl-encode",
    "jxl-decode"
);
regress!(
    regress_jxl_gray_f32,
    ImageFormat::Jxl,
    "jxl",
    gray_f32,
    "jxl-encode",
    "jxl-decode"
);

// JXL alpha-channel decode bug: zenjxl-decoder panics in frame/render.rs:1432
// when decoding images that were encoded with alpha-capable pixel formats.
// These tests are ignored until the decoder bug is fixed.
#[test]
#[ignore = "zenjxl-decoder alpha decode panic (render.rs:1432)"]
#[cfg(feature = "jxl-encode")]
#[cfg(feature = "jxl-decode")]
fn regress_jxl_rgba8() {
    let img = gradient_rgba8();
    check_roundtrip(
        EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_full_frame_rgba8(img.as_ref()),
        "jxl",
        "rgba8",
    );
}

#[test]
#[ignore = "zenjxl-decoder alpha decode panic (render.rs:1432)"]
#[cfg(feature = "jxl-encode")]
#[cfg(feature = "jxl-decode")]
fn regress_jxl_bgra8() {
    let img = gradient_bgra8();
    check_roundtrip(
        EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_full_frame_bgra8(img.as_ref()),
        "jxl",
        "bgra8",
    );
}

#[test]
#[ignore = "zenjxl-decoder alpha decode panic (render.rs:1432)"]
#[cfg(feature = "jxl-encode")]
#[cfg(feature = "jxl-decode")]
fn regress_jxl_bgrx8() {
    let img = gradient_bgra8();
    check_roundtrip(
        EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_full_frame_bgrx8(img.as_ref()),
        "jxl",
        "bgrx8",
    );
}

#[test]
#[ignore = "zenjxl-decoder alpha decode panic (render.rs:1432)"]
#[cfg(feature = "jxl-encode")]
#[cfg(feature = "jxl-decode")]
fn regress_jxl_rgba_f32() {
    let img = gradient_rgba_f32();
    check_roundtrip(
        EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_full_frame_rgba_f32(img.as_ref()),
        "jxl",
        "rgba_f32",
    );
}
