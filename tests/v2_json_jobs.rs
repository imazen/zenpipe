//! Integration tests for imageflow v2 JSON jobs executed end-to-end through
//! `execute_framewise`. Each test constructs a `Framewise`, round-trips it
//! through JSON serialization, and runs the pipeline with real image bytes.

#![cfg(all(
    feature = "imageflow-compat",
    feature = "nodes-jpeg",
    feature = "nodes-png"
))]

use std::collections::HashMap;

use imageflow_types::{
    Color, ColorFilterSrgb, ColorSrgb, CommandStringKind, Constraint, ConstraintGravity,
    ConstraintMode, EncoderPreset, ExecutionSecurity, Framewise, Node, PngBitDepth,
    RoundCornersMode,
};

// ═══════════════════════════════════════════════════════════════════════
// Test image generation
// ═══════════════════════════════════════════════════════════════════════

/// Create an 8x8 RGB JPEG for testing. Uses a simple gradient pattern.
fn make_test_jpeg() -> Vec<u8> {
    let w = 8u32;
    let h = 8u32;
    let bpp = 3usize; // RGB8
    let stride = w as usize * bpp;
    let mut pixels = vec![0u8; stride * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let offset = y * stride + x * bpp;
            pixels[offset] = (x * 32).min(255) as u8; // R
            pixels[offset + 1] = (y * 32).min(255) as u8; // G
            pixels[offset + 2] = 128; // B
        }
    }
    let ps =
        zenpixels::PixelSlice::new(&pixels, w, h, stride, zenpixels::PixelDescriptor::RGB8_SRGB)
            .expect("pixel slice");
    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode(ps, false)
        .expect("JPEG encode")
        .into_vec()
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Round-trip a Framewise through JSON and execute it with the given input.
fn run_framewise(framewise: &Framewise, input: &[u8]) -> ExecuteResult {
    // Serialize then deserialize — proves wire format round-trips.
    let json = serde_json::to_value(framewise).expect("serialize");
    let parsed: Framewise = serde_json::from_value(json).expect("deserialize");

    let io_buffers = HashMap::from([(0, input.to_vec())]);
    let security = ExecutionSecurity::sane_defaults();
    zenpipe::imageflow_compat::execute::execute_framewise(
        &parsed,
        &io_buffers,
        &security,
        &imageflow_types::JobOptions::default(),
    )
    .expect("execute_framewise")
}

/// Shorthand for the result type.
type ExecuteResult = zenpipe::imageflow_compat::execute::ExecuteResult;

/// Assert a single encode result with non-empty bytes.
fn assert_single_output(result: &ExecuteResult) -> &[u8] {
    assert_eq!(
        result.encode_results.len(),
        1,
        "expected exactly 1 encode result, got {}",
        result.encode_results.len()
    );
    let bytes = &result.encode_results[0].bytes;
    assert!(!bytes.is_empty(), "encoded output is empty");
    bytes
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Decode + Encode (passthrough)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_encode_passthrough() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    // Output should be a valid JPEG (starts with FFD8).
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "output is not a JPEG");
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Decode + Constrain + Encode (resize within)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_constrain_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Constrain(Constraint {
            mode: ConstraintMode::Within,
            w: Some(4),
            h: Some(4),
            hints: None,
            canvas_color: None,
            gravity: None,
        }),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(80),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    // Output dimensions should be <= 4x4.
    assert!(result.encode_results[0].width <= 4);
    assert!(result.encode_results[0].height <= 4);
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Decode + FlipH + Rotate90 + Encode (PNG)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_flip_rotate_encode_png() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::FlipH,
        Node::Rotate90,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    // PNG signature: 89 50 4E 47
    assert_eq!(
        &bytes[..4],
        &[0x89, 0x50, 0x4E, 0x47],
        "output is not a PNG"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. Decode + Crop + Encode (WebP lossy)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "nodes-webp")]
#[test]
fn decode_crop_encode_webp() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Crop {
            x1: 1,
            y1: 1,
            x2: 5,
            y2: 5,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::WebPLossy { quality: 75.0 },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    // WebP signature: RIFF....WEBP
    assert_eq!(&bytes[..4], b"RIFF", "output does not start with RIFF");
    assert_eq!(&bytes[8..12], b"WEBP", "output is not a WebP");
    assert!(result.encode_results[0].width <= 4);
    assert!(result.encode_results[0].height <= 4);
}

// ═══════════════════════════════════════════════════════════════════════
// 5. Decode + ExpandCanvas + Encode (Libpng)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_expand_canvas_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ExpandCanvas {
            left: 2,
            top: 2,
            right: 2,
            bottom: 2,
            color: Color::Black,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Libpng {
                depth: None,
                matte: None,
                zlib_compression: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
    // 8 + 2 + 2 = 12 in each dimension.
    assert_eq!(result.encode_results[0].width, 12);
    assert_eq!(result.encode_results[0].height, 12);
}

// ═══════════════════════════════════════════════════════════════════════
// 6. Decode + ColorFilterSrgb + Encode (multiple filter variants)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn color_filter_sepia() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorFilterSrgb(ColorFilterSrgb::Sepia),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn color_filter_grayscale_ntsc() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleNtsc),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn color_filter_invert() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorFilterSrgb(ColorFilterSrgb::Invert),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(90),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn color_filter_contrast() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorFilterSrgb(ColorFilterSrgb::Contrast(0.5)),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn color_filter_brightness() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorFilterSrgb(ColorFilterSrgb::Brightness(-0.2)),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 7. Decode + CropWhitespace + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_crop_whitespace_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::CropWhitespace {
            threshold: 80,
            percent_padding: 0.0,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 8. Decode + ApplyOrientation + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_apply_orientation_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ApplyOrientation { flag: 6 }, // 90 CW rotation
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 9. Decode + Transpose + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_transpose_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Transpose,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 10. Decode + RoundImageCorners + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_round_corners_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::RoundImageCorners {
            radius: RoundCornersMode::Percentage(20.0),
            background_color: Color::Transparent,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 11. Decode + WhiteBalanceHistogramAreaThresholdSrgb + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_white_balance_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::WhiteBalanceHistogramAreaThresholdSrgb {
            threshold: Some(0.06),
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 12. Decode + ColorMatrixSrgb (identity) + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_color_matrix_identity_encode() {
    let input = make_test_jpeg();
    let identity: [[f32; 5]; 5] = [
        [1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::ColorMatrixSrgb { matrix: identity },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(90),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 13. Decode + Constrain with FitCrop + Gravity + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_constrain_fit_crop_gravity_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Constrain(Constraint {
            mode: ConstraintMode::FitCrop,
            w: Some(4),
            h: Some(4),
            hints: None,
            canvas_color: None,
            gravity: Some(ConstraintGravity::Percentage { x: 75.0, y: 25.0 }),
        }),
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(85),
                progressive: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    assert!(result.encode_results[0].width <= 4);
    assert!(result.encode_results[0].height <= 4);
}

// ═══════════════════════════════════════════════════════════════════════
// 14. Decode + Resample2D + Encode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_resample2d_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Resample2D {
            w: 4,
            h: 4,
            hints: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
    assert_eq!(result.encode_results[0].width, 4);
    assert_eq!(result.encode_results[0].height, 4);
}

// ═══════════════════════════════════════════════════════════════════════
// 15. CommandString test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn command_string_resize_crop_png() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![Node::CommandString {
        kind: CommandStringKind::ImageResizer4,
        value: "w=4&h=4&mode=crop&format=png".to_string(),
        decode: Some(0),
        encode: Some(1),
        watermarks: None,
    }]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
}

// ═══════════════════════════════════════════════════════════════════════
// 16. EncoderPreset variants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn encoder_preset_mozjpeg() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Mozjpeg {
                quality: Some(75),
                progressive: Some(true),
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
}

#[test]
fn encoder_preset_libjpeg_turbo() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::LibjpegTurbo {
                quality: Some(80),
                progressive: None,
                optimize_huffman_coding: None,
                matte: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
}

#[test]
fn encoder_preset_libpng() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Libpng {
                depth: Some(PngBitDepth::Png32),
                matte: None,
                zlib_compression: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn encoder_preset_lodepng() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: Some(false),
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn encoder_preset_pngquant() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Pngquant {
                quality: Some(80),
                minimum_quality: Some(60),
                speed: Some(3),
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[cfg(feature = "nodes-webp")]
#[test]
fn encoder_preset_webp_lossy() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::WebPLossy { quality: 80.0 },
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
}

#[cfg(feature = "nodes-webp")]
#[test]
fn encoder_preset_webp_lossless() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::WebPLossless,
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
}

#[cfg(feature = "nodes-gif")]
#[test]
fn encoder_preset_gif() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Gif,
        },
    ]);
    let result = run_framewise(&fw, &input);
    let bytes = assert_single_output(&result);
    assert!(
        bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "output is not a GIF"
    );
}

#[test]
fn encoder_preset_auto() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Auto {
                quality_profile: imageflow_types::QualityProfile::Good,
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// 17. Graph mode: fan-out (one decode, two encodes)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn graph_fanout_two_encodes() {
    let input = make_test_jpeg();
    let fw = Framewise::Graph(imageflow_types::Graph {
        nodes: HashMap::from([
            (
                "0".into(),
                Node::Decode {
                    io_id: 0,
                    commands: None,
                },
            ),
            (
                "1".into(),
                Node::Constrain(Constraint {
                    mode: ConstraintMode::Within,
                    w: Some(4),
                    h: Some(4),
                    hints: None,
                    canvas_color: None,
                    gravity: None,
                }),
            ),
            (
                "2".into(),
                Node::Encode {
                    io_id: 1,
                    preset: EncoderPreset::Mozjpeg {
                        quality: Some(80),
                        progressive: None,
                        matte: None,
                    },
                },
            ),
            (
                "3".into(),
                Node::Encode {
                    io_id: 2,
                    preset: EncoderPreset::Lodepng {
                        maximum_deflate: None,
                    },
                },
            ),
        ]),
        edges: vec![
            imageflow_types::Edge {
                from: 0,
                to: 1,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 1,
                to: 2,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 1,
                to: 3,
                kind: imageflow_types::EdgeKind::Input,
            },
        ],
    });

    // Serialize + deserialize round-trip.
    let json = serde_json::to_value(&fw).expect("serialize");
    let parsed: Framewise = serde_json::from_value(json).expect("deserialize");

    let io_buffers = HashMap::from([(0, input)]);
    let security = ExecutionSecurity::sane_defaults();
    let result = zenpipe::imageflow_compat::execute::execute_framewise(
        &parsed,
        &io_buffers,
        &security,
        &imageflow_types::JobOptions::default(),
    )
    .expect("execute_framewise");

    assert_eq!(
        result.encode_results.len(),
        2,
        "expected 2 encode results for fan-out, got {}",
        result.encode_results.len()
    );
    for er in &result.encode_results {
        assert!(
            !er.bytes.is_empty(),
            "encode result for io_id {} is empty",
            er.io_id
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Additional geometry tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn decode_flip_v_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::FlipV,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn decode_rotate180_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Rotate180,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

#[test]
fn decode_rotate270_encode() {
    let input = make_test_jpeg();
    let fw = Framewise::Steps(vec![
        Node::Decode {
            io_id: 0,
            commands: None,
        },
        Node::Rotate270,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Lodepng {
                maximum_deflate: None,
            },
        },
    ]);
    let result = run_framewise(&fw, &input);
    assert_single_output(&result);
}

// ═══════════════════════════════════════════════════════════════════════
// JSON round-trip fidelity: verify complex structures survive serde
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn json_roundtrip_all_constraint_modes() {
    let modes = [
        ConstraintMode::Distort,
        ConstraintMode::Within,
        ConstraintMode::Fit,
        ConstraintMode::LargerThan,
        ConstraintMode::WithinCrop,
        ConstraintMode::FitCrop,
        ConstraintMode::AspectCrop,
        ConstraintMode::WithinPad,
        ConstraintMode::FitPad,
    ];
    for mode in &modes {
        let fw = Framewise::Steps(vec![
            Node::Decode {
                io_id: 0,
                commands: None,
            },
            Node::Constrain(Constraint {
                mode: *mode,
                w: Some(4),
                h: Some(4),
                hints: None,
                canvas_color: None,
                gravity: None,
            }),
            Node::Encode {
                io_id: 1,
                preset: EncoderPreset::Lodepng {
                    maximum_deflate: None,
                },
            },
        ]);
        let json = serde_json::to_value(&fw).expect("serialize");
        let parsed: Framewise = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(fw, parsed, "JSON round-trip failed for mode {:?}", mode);
    }
}

#[test]
fn json_roundtrip_color_variants() {
    let colors = [
        Color::Transparent,
        Color::Black,
        Color::Srgb(ColorSrgb::Hex("FF0000FF".to_string())),
    ];
    for color in &colors {
        let fw = Framewise::Steps(vec![
            Node::Decode {
                io_id: 0,
                commands: None,
            },
            Node::ExpandCanvas {
                left: 1,
                top: 1,
                right: 1,
                bottom: 1,
                color: color.clone(),
            },
            Node::Encode {
                io_id: 1,
                preset: EncoderPreset::Lodepng {
                    maximum_deflate: None,
                },
            },
        ]);
        let json = serde_json::to_value(&fw).expect("serialize");
        let parsed: Framewise = serde_json::from_value(json).expect("deserialize");
        assert_eq!(fw, parsed, "JSON round-trip failed for color {:?}", color);
    }
}

#[test]
fn json_roundtrip_encoder_presets() {
    let presets = vec![
        EncoderPreset::Mozjpeg {
            quality: Some(85),
            progressive: Some(true),
            matte: None,
        },
        EncoderPreset::LibjpegTurbo {
            quality: Some(90),
            progressive: None,
            optimize_huffman_coding: Some(true),
            matte: None,
        },
        EncoderPreset::Libpng {
            depth: Some(PngBitDepth::Png32),
            matte: None,
            zlib_compression: Some(6),
        },
        EncoderPreset::Lodepng {
            maximum_deflate: Some(true),
        },
        EncoderPreset::Pngquant {
            quality: Some(80),
            minimum_quality: Some(60),
            speed: Some(3),
            maximum_deflate: None,
        },
        EncoderPreset::WebPLossy { quality: 75.0 },
        EncoderPreset::WebPLossless,
        EncoderPreset::Gif,
        EncoderPreset::Auto {
            quality_profile: imageflow_types::QualityProfile::Good,
            quality_profile_dpr: Some(2.0),
            matte: None,
            lossless: None,
            allow: None,
        },
    ];
    for preset in &presets {
        let fw = Framewise::Steps(vec![
            Node::Decode {
                io_id: 0,
                commands: None,
            },
            Node::Encode {
                io_id: 1,
                preset: preset.clone(),
            },
        ]);
        let json = serde_json::to_value(&fw).expect("serialize");
        let parsed: Framewise = serde_json::from_value(json).expect("deserialize");
        assert_eq!(fw, parsed, "JSON round-trip failed for preset {:?}", preset);
    }
}

#[test]
fn json_roundtrip_round_corners_modes() {
    let modes = [
        RoundCornersMode::Percentage(20.0),
        RoundCornersMode::Pixels(5.0),
        RoundCornersMode::Circle,
        RoundCornersMode::PercentageCustom {
            top_left: 10.0,
            top_right: 20.0,
            bottom_right: 30.0,
            bottom_left: 40.0,
        },
        RoundCornersMode::PixelsCustom {
            top_left: 1.0,
            top_right: 2.0,
            bottom_right: 3.0,
            bottom_left: 4.0,
        },
    ];
    for mode in &modes {
        let fw = Framewise::Steps(vec![
            Node::Decode {
                io_id: 0,
                commands: None,
            },
            Node::RoundImageCorners {
                radius: *mode,
                background_color: Color::Transparent,
            },
            Node::Encode {
                io_id: 1,
                preset: EncoderPreset::Lodepng {
                    maximum_deflate: None,
                },
            },
        ]);
        let json = serde_json::to_value(&fw).expect("serialize");
        let parsed: Framewise = serde_json::from_value(json).expect("deserialize");
        assert_eq!(
            fw, parsed,
            "JSON round-trip failed for corners mode {:?}",
            mode
        );
    }
}

#[test]
fn json_roundtrip_graph_mode() {
    let fw = Framewise::Graph(imageflow_types::Graph {
        nodes: HashMap::from([
            (
                "0".into(),
                Node::Decode {
                    io_id: 0,
                    commands: None,
                },
            ),
            (
                "1".into(),
                Node::Constrain(Constraint {
                    mode: ConstraintMode::Within,
                    w: Some(4),
                    h: Some(4),
                    hints: None,
                    canvas_color: None,
                    gravity: None,
                }),
            ),
            (
                "2".into(),
                Node::Encode {
                    io_id: 1,
                    preset: EncoderPreset::Mozjpeg {
                        quality: Some(80),
                        progressive: None,
                        matte: None,
                    },
                },
            ),
        ]),
        edges: vec![
            imageflow_types::Edge {
                from: 0,
                to: 1,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 1,
                to: 2,
                kind: imageflow_types::EdgeKind::Input,
            },
        ],
    });
    let json = serde_json::to_value(&fw).expect("serialize");
    let parsed: Framewise = serde_json::from_value(json).expect("deserialize");
    assert_eq!(fw, parsed);
}

// ═══════════════════════════════════════════════════════════════════════
// Graph mode: multi-input compositing (W5 — canvas + input edges)
// ═══════════════════════════════════════════════════════════════════════

/// Encode a solid-color RGBA8 PNG fixture.
fn solid_png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        pixels.extend_from_slice(&rgba);
    }
    let slice =
        zenpixels::PixelSlice::new(&pixels, w, h, (w * 4) as usize, zenpipe::format::RGBA8_SRGB)
            .unwrap();
    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Png)
        .encode(slice, true)
        .expect("fixture png")
        .data()
        .to_vec()
}

/// Decode PNG output and sample the pixel at (x, y) as RGBA.
fn sample_png(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let decoded = zencodecs::DecodeRequest::new(data)
        .decode_full_frame()
        .expect("decode output");
    let px = decoded.pixels();
    let bytes = px.as_strided_bytes();
    let desc = px.descriptor();
    let bpp = desc.bytes_per_pixel();
    let stride = px.stride();
    let off = y as usize * stride + x as usize * bpp;
    let mut out = [0u8, 0, 0, 255];
    out[..bpp.min(4)].copy_from_slice(&bytes[off..off + bpp.min(4)]);
    out
}

fn png_encode_node(io_id: i32) -> Node {
    Node::Encode {
        io_id,
        preset: EncoderPreset::Lodepng {
            maximum_deflate: None,
        },
    }
}

#[test]
fn graph_draw_image_exact_composites_input_edge() {
    // 32×32 red base (canvas edge) + 8×8 blue logo (input edge), drawn at
    // (4,4) scaled to 16×16, compose mode. imageflow graph shape verbatim.
    let base = solid_png(32, 32, [255, 0, 0, 255]);
    let logo = solid_png(8, 8, [0, 0, 255, 255]);

    let fw = Framewise::Graph(imageflow_types::Graph {
        nodes: HashMap::from([
            (
                "0".into(),
                Node::Decode {
                    io_id: 0,
                    commands: None,
                },
            ),
            (
                "1".into(),
                Node::Decode {
                    io_id: 1,
                    commands: None,
                },
            ),
            (
                "2".into(),
                Node::DrawImageExact {
                    x: 4,
                    y: 4,
                    w: 16,
                    h: 16,
                    blend: Some(imageflow_types::CompositingMode::Compose),
                    hints: None,
                },
            ),
            ("3".into(), png_encode_node(2)),
        ]),
        edges: vec![
            imageflow_types::Edge {
                from: 0,
                to: 2,
                kind: imageflow_types::EdgeKind::Canvas,
            },
            imageflow_types::Edge {
                from: 1,
                to: 2,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 2,
                to: 3,
                kind: imageflow_types::EdgeKind::Input,
            },
        ],
    });

    // JSON round-trip first — this is the wire shape we guarantee.
    let json = serde_json::to_value(&fw).expect("serialize");
    let parsed: Framewise = serde_json::from_value(json).expect("deserialize");

    let io_buffers = HashMap::from([(0, base), (1, logo)]);
    let result = zenpipe::imageflow_compat::execute::execute_framewise(
        &parsed,
        &io_buffers,
        &ExecutionSecurity::sane_defaults(),
        &imageflow_types::JobOptions::default(),
    )
    .expect("multi-input graph must execute");

    assert_eq!(result.encode_results.len(), 1);
    let out = &result.encode_results[0];
    assert_eq!(out.io_id, 2);
    assert_eq!(
        (out.width, out.height),
        (32, 32),
        "canvas defines output dims"
    );

    // Outside the drawn box: red. Inside: blue (opaque logo, compose = replace).
    let corner = sample_png(&out.bytes, 0, 0);
    assert!(
        corner[0] > 200 && corner[2] < 60,
        "corner must stay red, got {corner:?}"
    );
    let inside = sample_png(&out.bytes, 10, 10);
    assert!(
        inside[2] > 200 && inside[0] < 60,
        "logo area must be blue, got {inside:?}"
    );
    // The logo was resized 8×8 → 16×16, so (18,18) is just inside its box.
    let edge = sample_png(&out.bytes, 18, 18);
    assert!(
        edge[2] > 200,
        "resized logo must cover (18,18), got {edge:?}"
    );
    let outside = sample_png(&out.bytes, 24, 24);
    assert!(
        outside[0] > 200,
        "outside the 4..20 box must be red, got {outside:?}"
    );
}

#[test]
fn graph_copy_rect_to_canvas_onto_created_canvas() {
    // CreateCanvas 32×32 white (canvas edge), decode 16×16 blue (input edge),
    // copy the (0,0,8,8) window to (10,10). Overwrite semantics, no resize.
    let blue = solid_png(16, 16, [0, 0, 255, 255]);

    let fw = Framewise::Graph(imageflow_types::Graph {
        nodes: HashMap::from([
            (
                "0".into(),
                Node::CreateCanvas {
                    format: imageflow_types::PixelFormat::Bgra32,
                    w: 32,
                    h: 32,
                    color: imageflow_types::Color::Srgb(imageflow_types::ColorSrgb::Hex(
                        "FFFFFFFF".into(),
                    )),
                },
            ),
            (
                "1".into(),
                Node::Decode {
                    io_id: 0,
                    commands: None,
                },
            ),
            (
                "2".into(),
                Node::CopyRectToCanvas {
                    from_x: 0,
                    from_y: 0,
                    w: 8,
                    h: 8,
                    x: 10,
                    y: 10,
                },
            ),
            ("3".into(), png_encode_node(1)),
        ]),
        edges: vec![
            imageflow_types::Edge {
                from: 0,
                to: 2,
                kind: imageflow_types::EdgeKind::Canvas,
            },
            imageflow_types::Edge {
                from: 1,
                to: 2,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 2,
                to: 3,
                kind: imageflow_types::EdgeKind::Input,
            },
        ],
    });

    let json = serde_json::to_value(&fw).expect("serialize");
    let parsed: Framewise = serde_json::from_value(json).expect("deserialize");

    let io_buffers = HashMap::from([(0, blue)]);
    let result = zenpipe::imageflow_compat::execute::execute_framewise(
        &parsed,
        &io_buffers,
        &ExecutionSecurity::sane_defaults(),
        &imageflow_types::JobOptions::default(),
    )
    .expect("copy_rect graph must execute");

    let out = &result.encode_results[0];
    assert_eq!((out.width, out.height), (32, 32));
    let pasted = sample_png(&out.bytes, 11, 11);
    assert!(
        pasted[2] > 200 && pasted[0] < 60,
        "pasted window must be blue, got {pasted:?}"
    );
    let white = sample_png(&out.bytes, 0, 0);
    assert!(
        white[0] > 200 && white[1] > 200 && white[2] > 200,
        "canvas must stay white, got {white:?}"
    );
    let below = sample_png(&out.bytes, 19, 19);
    assert!(
        below[0] > 200 && below[1] > 200 && below[2] > 200,
        "outside the 8×8 paste (10..18) must be white, got {below:?}"
    );
}

#[test]
fn graph_nested_composites_chain() {
    // Two chained composites on one spine: logo onto base, then badge onto
    // the result — the spine carries two input-edge resolutions.
    let base = solid_png(32, 32, [255, 0, 0, 255]);
    let logo = solid_png(8, 8, [0, 0, 255, 255]);
    let badge = solid_png(4, 4, [0, 255, 0, 255]);

    let fw = Framewise::Graph(imageflow_types::Graph {
        nodes: HashMap::from([
            (
                "0".into(),
                Node::Decode {
                    io_id: 0,
                    commands: None,
                },
            ),
            (
                "1".into(),
                Node::Decode {
                    io_id: 1,
                    commands: None,
                },
            ),
            (
                "2".into(),
                Node::Decode {
                    io_id: 2,
                    commands: None,
                },
            ),
            (
                "3".into(),
                Node::DrawImageExact {
                    x: 0,
                    y: 0,
                    w: 8,
                    h: 8,
                    blend: Some(imageflow_types::CompositingMode::Compose),
                    hints: None,
                },
            ),
            (
                "4".into(),
                Node::DrawImageExact {
                    x: 24,
                    y: 24,
                    w: 4,
                    h: 4,
                    blend: Some(imageflow_types::CompositingMode::Compose),
                    hints: None,
                },
            ),
            ("5".into(), png_encode_node(3)),
        ]),
        edges: vec![
            imageflow_types::Edge {
                from: 0,
                to: 3,
                kind: imageflow_types::EdgeKind::Canvas,
            },
            imageflow_types::Edge {
                from: 1,
                to: 3,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 3,
                to: 4,
                kind: imageflow_types::EdgeKind::Canvas,
            },
            imageflow_types::Edge {
                from: 2,
                to: 4,
                kind: imageflow_types::EdgeKind::Input,
            },
            imageflow_types::Edge {
                from: 4,
                to: 5,
                kind: imageflow_types::EdgeKind::Input,
            },
        ],
    });

    let io_buffers = HashMap::from([(0, base), (1, logo), (2, badge)]);
    let result = zenpipe::imageflow_compat::execute::execute_framewise(
        &fw,
        &io_buffers,
        &ExecutionSecurity::sane_defaults(),
        &imageflow_types::JobOptions::default(),
    )
    .expect("nested composite graph must execute");

    let out = &result.encode_results[0];
    let logo_px = sample_png(&out.bytes, 3, 3);
    assert!(logo_px[2] > 200, "logo at (3,3), got {logo_px:?}");
    let badge_px = sample_png(&out.bytes, 26, 26);
    assert!(
        badge_px[1] > 200 && badge_px[2] < 60,
        "badge at (26,26), got {badge_px:?}"
    );
    let base_px = sample_png(&out.bytes, 16, 16);
    assert!(base_px[0] > 200, "base at (16,16), got {base_px:?}");
}

#[test]
fn graph_imageflow_canonical_example_executes() {
    // imageflow's own Framewise::example_graph() — decode, create_canvas,
    // copy_rect_to_canvas, resample_2d, two encodes. THE canonical graph
    // from the imageflow docs; compatibility means this runs.
    let fw = Framewise::example_graph();
    let input = make_test_jpeg();
    let io_buffers = HashMap::from([(0, input)]);

    let result = zenpipe::imageflow_compat::execute::execute_framewise(
        &fw,
        &io_buffers,
        &ExecutionSecurity::sane_defaults(),
        &imageflow_types::JobOptions::default(),
    )
    .expect("imageflow's canonical example graph must execute");
    assert!(
        !result.encode_results.is_empty(),
        "example graph must produce encodes"
    );
    for er in &result.encode_results {
        assert!(!er.bytes.is_empty());
    }
}
