//! Tests for animation support — frame-by-frame decode, process, encode.
//!
//! Uses zengif for end-to-end testing: generate animated GIF → decode
//! frame-by-frame → process through pipeline → encode back to GIF.

use std::borrow::Cow;

use hashbrown::HashMap;
use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
use zengif::{GifDecoderConfig, GifEncoderConfig};
use zenpixels::PixelDescriptor;

use zenpipe::Source;
use zenpipe::animation::{FrameSink, FrameSource};
use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::CallbackSource;

/// Build a simple animated GIF with solid-color frames.
fn build_test_gif(frame_count: usize, width: u16, height: u16) -> Vec<u8> {
    let config = GifEncoderConfig::new();
    let mut encoder = config
        .job()
        .with_canvas_size(width as u32, height as u32)
        .animation_frame_encoder()
        .unwrap();

    let bpp = 4usize;
    let stride = width as usize * bpp;

    for i in 0..frame_count {
        let gray = ((i + 1) * 255 / frame_count).min(255) as u8;
        let mut pixels = vec![0u8; stride * height as usize];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[gray, gray, gray, 255]);
        }
        let slice = zenpixels::PixelSlice::new(
            &pixels,
            width as u32,
            height as u32,
            stride,
            PixelDescriptor::RGBA8_SRGB,
        )
        .unwrap();
        encoder.push_frame(slice, 100, None).unwrap();
    }

    let output = encoder.finish(None).unwrap();
    output.into_vec()
}

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

fn make_gif_decoder(data: Vec<u8>) -> Box<dyn zencodec::decode::DynAnimationFrameDecoder> {
    let dec = GifDecoderConfig::new();
    dec.job()
        .dyn_animation_frame_decoder(Cow::Owned(data), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap()
}

fn make_gif_encoder(
    width: u16,
    height: u16,
) -> Box<dyn zencodec::encode::DynAnimationFrameEncoder> {
    let config = GifEncoderConfig::new();
    config
        .job()
        .with_canvas_size(width as u32, height as u32)
        .dyn_animation_frame_encoder()
        .unwrap()
}

// =========================================================================
// FrameSource tests
// =========================================================================

#[test]
fn frame_source_decode_3_frames() {
    let gif_data = build_test_gif(3, 8, 8);
    let mut source = FrameSource::new(make_gif_decoder(gif_data)).unwrap();

    let info = source.frame_info().expect("first frame should be loaded");
    assert_eq!(info.index, 0);
    assert_eq!(info.duration_ms, 100);
    assert_eq!(source.width(), 8);
    assert_eq!(source.height(), 8);

    let frame_count = 3usize;
    let frame0_pixels = drain(&mut source);
    assert_pixel_gray(&frame0_pixels, 0, frame_count);

    assert!(source.advance_frame().unwrap());
    let info = source.frame_info().unwrap();
    assert_eq!(info.index, 1);
    let frame1_pixels = drain(&mut source);
    assert_pixel_gray(&frame1_pixels, 1, frame_count);

    assert!(source.advance_frame().unwrap());
    let info = source.frame_info().unwrap();
    assert_eq!(info.index, 2);
    let frame2_pixels = drain(&mut source);
    assert_pixel_gray(&frame2_pixels, 2, frame_count);

    assert!(!source.advance_frame().unwrap());
    assert!(source.frame_info().is_none());
}

#[test]
fn frame_source_strips_cover_frame() {
    let gif_data = build_test_gif(1, 4, 50);
    let mut source = FrameSource::new(make_gif_decoder(gif_data)).unwrap();

    let mut total_rows = 0u32;
    let mut strip_count = 0u32;
    while let Ok(Some(strip)) = source.next() {
        assert_eq!(strip.width(), 4);
        total_rows += strip.rows();
        strip_count += 1;
    }
    assert_eq!(total_rows, 50);
    assert_eq!(strip_count, 4); // 16+16+16+2
}

// =========================================================================
// FrameSink tests
// =========================================================================

#[test]
fn frame_sink_encode_2_frames() {
    let width = 4u16;
    let height = 4u16;

    let mut sink = FrameSink::new(
        make_gif_encoder(width, height),
        width as u32,
        height as u32,
        format::RGBA8_SRGB,
    );

    // Frame 1: red
    sink.begin_frame();
    let mut src = make_solid_source(width as u32, height as u32, [255, 0, 0, 255]);
    zenpipe::execute(src.as_mut(), &mut sink).unwrap();
    sink.finish_frame(100).unwrap();

    // Frame 2: blue
    sink.begin_frame();
    let mut src = make_solid_source(width as u32, height as u32, [0, 0, 255, 255]);
    zenpipe::execute(src.as_mut(), &mut sink).unwrap();
    sink.finish_frame(200).unwrap();

    let output = sink.finish_animation().unwrap();
    let encoded = output.into_vec();
    assert!(encoded.len() > 10);

    // Verify 2 frames.
    let dec = GifDecoderConfig::new();
    let mut verify = dec
        .job()
        .animation_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    // Frame 0: should be reddish
    let frame0 = verify.render_next_frame_owned(None).unwrap().expect("missing frame 0");
    let px0 = frame0.pixels().as_strided_bytes();
    // Sample first pixel (RGBA)
    assert!(px0.len() >= 4, "frame 0 pixel data too short");
    let (r0, g0, b0) = (px0[0], px0[1], px0[2]);
    assert!(
        r0 > 200 && g0 < 50 && b0 < 50,
        "frame 0 should be reddish, got r={r0} g={g0} b={b0}"
    );

    // Frame 1: should be bluish
    let frame1 = verify.render_next_frame_owned(None).unwrap().expect("missing frame 1");
    let px1 = frame1.pixels().as_strided_bytes();
    assert!(px1.len() >= 4, "frame 1 pixel data too short");
    let (r1, g1, b1) = (px1[0], px1[1], px1[2]);
    assert!(
        b1 > 200 && r1 < 50 && g1 < 50,
        "frame 1 should be bluish, got r={r1} g={g1} b={b1}"
    );

    assert!(verify.render_next_frame_owned(None).unwrap().is_none());
}

// =========================================================================
// transcode() end-to-end
// =========================================================================

#[test]
fn transcode_gif_passthrough() {
    let gif_data = build_test_gif(3, 8, 8);

    let output = zenpipe::animation::transcode(
        make_gif_decoder(gif_data),
        make_gif_encoder(8, 8),
        8,
        8,
        format::RGBA8_SRGB,
        |frame_src, _idx| Ok(frame_src),
    )
    .unwrap();

    let encoded = output.into_vec();

    // Verify 3 frames.
    let dec = GifDecoderConfig::new();
    let mut verify = dec
        .job()
        .animation_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    let frame_count = 3usize;
    for i in 0..frame_count {
        let frame = verify
            .render_next_frame_owned(None)
            .unwrap()
            .unwrap_or_else(|| panic!("missing frame {i}"));
        let px = frame.pixels().as_strided_bytes();
        assert!(px.len() >= 4, "frame {i} pixel data too short");

        // build_test_gif produces gray = (i+1)*255/frame_count for each frame
        let expected_gray = ((i + 1) * 255 / frame_count) as u8;
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        let tolerance = 32i16; // GIF quantization loses precision
        assert!(
            (r as i16 - expected_gray as i16).abs() <= tolerance
                && (g as i16 - expected_gray as i16).abs() <= tolerance
                && (b as i16 - expected_gray as i16).abs() <= tolerance,
            "frame {i}: expected ~gray({expected_gray}), got r={r} g={g} b={b}"
        );
        assert!(a > 200, "frame {i}: alpha should be opaque, got a={a}");
    }
    assert!(verify.render_next_frame_owned(None).unwrap().is_none());
}

#[test]
fn transcode_gif_with_crop() {
    let gif_data = build_test_gif(2, 8, 8);

    let output = zenpipe::animation::transcode(
        make_gif_decoder(gif_data),
        make_gif_encoder(4, 4),
        4,
        4,
        format::RGBA8_SRGB,
        |frame_src, _idx| {
            let mut g = PipelineGraph::new();
            let src = g.add_node(NodeOp::Source);
            let crop = g.add_node(NodeOp::Crop {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
            });
            let out = g.add_node(NodeOp::Output);
            g.add_edge(src, crop, EdgeKind::Input);
            g.add_edge(crop, out, EdgeKind::Input);

            let mut sources = HashMap::new();
            sources.insert(src, frame_src);
            g.compile(sources)
        },
    )
    .unwrap();

    let encoded = output.into_vec();

    let dec = GifDecoderConfig::new();
    let mut verify = dec
        .job()
        .animation_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    assert_eq!(verify.info().width, 4);
    assert_eq!(verify.info().height, 4);

    // Solid-color frames survive cropping — verify pixel values still match
    let crop_frame_count = 2usize;
    for i in 0..crop_frame_count {
        let frame = verify
            .render_next_frame_owned(None)
            .unwrap()
            .unwrap_or_else(|| panic!("missing cropped frame {i}"));
        let px = frame.pixels().as_strided_bytes();
        assert!(px.len() >= 4, "cropped frame {i} pixel data too short");

        let expected_gray = ((i + 1) * 255 / crop_frame_count) as u8;
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        let tolerance = 32i16;
        assert!(
            (r as i16 - expected_gray as i16).abs() <= tolerance
                && (g as i16 - expected_gray as i16).abs() <= tolerance
                && (b as i16 - expected_gray as i16).abs() <= tolerance,
            "cropped frame {i}: expected ~gray({expected_gray}), got r={r} g={g} b={b}"
        );
        assert!(a > 200, "cropped frame {i}: alpha should be opaque, got a={a}");
    }
    assert!(verify.render_next_frame_owned(None).unwrap().is_none());
}

// =========================================================================
// Helpers
// =========================================================================

/// Assert that the first pixel of RGBA frame data matches the expected gray value
/// from `build_test_gif`: gray = (frame_index+1)*255/frame_count.
/// Allows ±32 tolerance for GIF palette quantization.
fn assert_pixel_gray(pixel_data: &[u8], frame_index: usize, frame_count: usize) {
    assert!(
        pixel_data.len() >= 4,
        "frame {frame_index} pixel data too short ({} bytes)",
        pixel_data.len()
    );
    let expected_gray = ((frame_index + 1) * 255 / frame_count) as u8;
    let (r, g, b, a) = (pixel_data[0], pixel_data[1], pixel_data[2], pixel_data[3]);
    let tolerance = 32i16;
    assert!(
        (r as i16 - expected_gray as i16).abs() <= tolerance
            && (g as i16 - expected_gray as i16).abs() <= tolerance
            && (b as i16 - expected_gray as i16).abs() <= tolerance,
        "frame {frame_index}: expected ~gray({expected_gray}), got r={r} g={g} b={b}"
    );
    assert!(
        a > 200,
        "frame {frame_index}: alpha should be opaque, got a={a}"
    );
}

fn make_solid_source(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}
