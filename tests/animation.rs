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
        for px in pixels.as_chunks_mut::<4>().0.iter_mut() {
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
    let frame0 = verify
        .render_next_frame_owned(None)
        .unwrap()
        .expect("missing frame 0");
    let px0 = frame0.pixels().as_strided_bytes();
    // Sample first pixel (RGBA)
    assert!(px0.len() >= 4, "frame 0 pixel data too short");
    let (r0, g0, b0) = (px0[0], px0[1], px0[2]);
    assert!(
        r0 > 200 && g0 < 50 && b0 < 50,
        "frame 0 should be reddish, got r={r0} g={g0} b={b0}"
    );

    // Frame 1: should be bluish
    let frame1 = verify
        .render_next_frame_owned(None)
        .unwrap()
        .expect("missing frame 1");
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
        assert!(
            a > 200,
            "cropped frame {i}: alpha should be opaque, got a={a}"
        );
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
            for px in buf[..row_bytes].as_chunks_mut::<4>().0.iter_mut() {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

// =========================================================================
// ImageJob animated routing (W8): processing nodes apply per frame
// =========================================================================

// Needs the GIF codec (decode + animated encode) and the PNG encoder for the
// animated→static fallback; without them the probe fails with
// `DisabledFormat(Gif)` before any routing happens.
#[cfg(all(feature = "job", feature = "nodes-gif", feature = "nodes-png"))]
mod imagejob_animation {
    use super::*;
    use zenpipe::job::ImageJob;

    fn resize_node(w: u32, h: u32) -> Box<dyn zennode::NodeInstance> {
        let registry = zenpipe::full_registry();
        let def = registry.get("zenresize.constrain").unwrap();
        let mut params = zennode::ParamMap::new();
        params.insert("w".into(), zennode::ParamValue::U32(w));
        params.insert("h".into(), zennode::ParamValue::U32(h));
        params.insert("mode".into(), zennode::ParamValue::Str("fit".into()));
        def.create(&params).unwrap()
    }

    fn count_frames(gif: &[u8]) -> usize {
        let dec = GifDecoderConfig::new();
        let mut verify = dec
            .job()
            .animation_frame_decoder(
                std::borrow::Cow::Owned(gif.to_vec()),
                &[PixelDescriptor::RGBA8_SRGB],
            )
            .unwrap();
        let mut n = 0;
        while verify.render_next_frame_owned(None).unwrap().is_some() {
            n += 1;
        }
        n
    }

    #[test]
    fn resize_gif_preserves_all_frames() {
        // "Resize this GIF" must stay an animated GIF — the former behavior
        // silently returned a single-frame still (Known Bug W8).
        let gif_data = build_test_gif(3, 8, 8);
        let nodes = vec![resize_node(4, 4)];

        let result = ImageJob::new()
            .add_input(0, gif_data)
            .add_output(1)
            .with_nodes(&nodes)
            .with_output_extension("gif")
            .run()
            .expect("animated resize job");

        let er = &result.encode_results[0];
        assert_eq!((er.width, er.height), (4, 4), "frames must be resized");
        assert_eq!(count_frames(&er.bytes), 3, "all frames must survive");
    }

    #[test]
    fn gif_no_nodes_still_reencodes_all_frames() {
        let gif_data = build_test_gif(2, 8, 8);
        let result = ImageJob::new()
            .add_input(0, gif_data)
            .add_output(1)
            .with_output_extension("gif")
            .run()
            .expect("animated passthrough job");
        assert_eq!(count_frames(&result.encode_results[0].bytes), 2);
    }

    #[test]
    fn animated_to_static_target_takes_first_frame() {
        // Static formats have no animation encoder — documented fallback.
        let gif_data = build_test_gif(3, 8, 8);
        let result = ImageJob::new()
            .add_input(0, gif_data)
            .add_output(1)
            .with_output_extension("png")
            .run()
            .expect("animated→static job");
        let er = &result.encode_results[0];
        assert_eq!(&er.bytes[..4], b"\x89PNG");
    }
}

/// A strip in another pixel format is refused, not sliced: an RGB8 row is
/// shorter than an RGBA8 sink's row.
#[test]
fn frame_sink_refuses_a_strip_in_another_format() {
    use zenpipe::Sink as _;
    let mut sink = FrameSink::new(make_gif_encoder(4, 2), 4, 2, format::RGBA8_SRGB);
    sink.begin_frame();
    let rgb = vec![7u8; 4 * 2 * 3];
    let strip = zenpixels::PixelSlice::new(&rgb, 4, 2, 4 * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let err = sink
        .consume(&strip)
        .expect_err("an RGB8 strip in an RGBA8 frame sink must be refused");
    assert!(
        matches!(err.error(), zenpipe::PipeError::FormatMismatch { .. }),
        "expected FormatMismatch, got {err}"
    );
}

const SINK_W: u32 = 4;
const SINK_H: u32 = 2;

/// Two rows of four opaque RGBA8 pixels in three colors, which GIF keeps exactly.
fn sink_rgba() -> Vec<u8> {
    let colors = [[200u8, 20, 20, 255], [20, 200, 20, 255], [20, 20, 200, 255]];
    (0..SINK_W * SINK_H)
        .flat_map(|i| colors[(i % 3) as usize])
        .collect()
}

/// `packed` rows of `row_bytes` each, followed by `pad` garbage bytes.
fn padded(packed: &[u8], row_bytes: usize, pad: usize) -> Vec<u8> {
    packed
        .chunks(row_bytes)
        .flat_map(|row| row.iter().copied().chain(core::iter::repeat_n(0xEE, pad)))
        .collect()
}

fn open_frame_sink() -> FrameSink {
    let mut sink = FrameSink::new(
        make_gif_encoder(SINK_W as u16, SINK_H as u16),
        SINK_W,
        SINK_H,
        format::RGBA8_SRGB,
    );
    sink.begin_frame();
    sink
}

/// The RGBA8 pixels of a GIF's first frame.
fn first_gif_frame(gif: &[u8]) -> Vec<u8> {
    let mut decoder = GifDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Owned(gif.to_vec()), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();
    decoder
        .render_next_frame_owned(None)
        .unwrap()
        .expect("one frame")
        .into_buffer()
        .copy_to_contiguous_bytes()
}

/// Give the sink the valid frame and return what its encoder wrote.
fn finish_with_valid_frame(mut sink: FrameSink) -> Vec<u8> {
    use zenpipe::Sink as _;
    let rgba = sink_rgba();
    let strip = zenpixels::PixelSlice::new(
        &rgba,
        SINK_W,
        SINK_H,
        SINK_W as usize * 4,
        PixelDescriptor::RGBA8_SRGB,
    )
    .unwrap();
    sink.consume(&strip).unwrap();
    sink.finish_frame(100).unwrap();
    sink.finish_animation().unwrap().into_vec()
}

/// Strided RGBA8 rows are read by width, never into their padding.
#[test]
fn frame_sink_reads_strided_rgba_rows_exactly() {
    use zenpipe::Sink as _;
    let rgba = sink_rgba();
    let rows = padded(&rgba, SINK_W as usize * 4, 12);
    let strip = zenpixels::PixelSlice::new(
        &rows,
        SINK_W,
        SINK_H,
        SINK_W as usize * 4 + 12,
        PixelDescriptor::RGBA8_SRGB,
    )
    .unwrap();
    let mut sink = open_frame_sink();
    sink.consume(&strip).unwrap();
    sink.finish_frame(100).unwrap();
    let gif = sink.finish_animation().unwrap().into_vec();
    assert_eq!(first_gif_frame(&gif), rgba);
}

/// A strided RGB8 strip is refused before the sink writes anything: the valid
/// frame that follows encodes exactly.
#[test]
fn frame_sink_refuses_strided_rgb_before_writing() {
    use zenpipe::Sink as _;
    let rgb: Vec<u8> = sink_rgba()
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let rows = padded(&rgb, SINK_W as usize * 3, 6);
    let strip = zenpixels::PixelSlice::new(
        &rows,
        SINK_W,
        SINK_H,
        SINK_W as usize * 3 + 6,
        PixelDescriptor::RGB8_SRGB,
    )
    .unwrap();
    let mut sink = open_frame_sink();
    let err = sink
        .consume(&strip)
        .expect_err("an RGB8 strip must be refused by an RGBA8 frame sink");
    assert!(
        matches!(err.error(), zenpipe::PipeError::FormatMismatch { .. }),
        "expected FormatMismatch, got {err}"
    );
    assert_eq!(first_gif_frame(&finish_with_valid_frame(sink)), sink_rgba());
}

/// Narrower and wider strips are refused before the sink writes anything.
#[test]
fn frame_sink_refuses_other_widths_before_writing() {
    use zenpipe::Sink as _;
    let mut sink = open_frame_sink();
    for width in [SINK_W - 1, SINK_W + 1] {
        let pixels = vec![9u8; width as usize * 4 * SINK_H as usize];
        let strip = zenpixels::PixelSlice::new(
            &pixels,
            width,
            SINK_H,
            width as usize * 4,
            PixelDescriptor::RGBA8_SRGB,
        )
        .unwrap();
        let err = sink
            .consume(&strip)
            .expect_err("a strip of another width must be refused");
        assert!(
            matches!(err.error(), zenpipe::PipeError::DimensionMismatch(_)),
            "width {width}: expected DimensionMismatch, got {err}"
        );
    }
    assert_eq!(first_gif_frame(&finish_with_valid_frame(sink)), sink_rgba());
}

// =========================================================================
// Animated WebP: every differing frame and its duration survive the codec
// and ImageJob's animated route, into and out of WebP. zenwebp 8d40b11 made a
// one-frame animation encode as a still; nothing here may lose a frame.
// =========================================================================

#[cfg(all(feature = "job", feature = "nodes-webp", feature = "nodes-gif"))]
mod animated_webp_frames {
    use super::*;
    use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
    use zenpipe::job::ImageJob;

    const W: u32 = 16;
    const H: u32 = 12;
    /// One duration per frame; GIF's centisecond clock keeps each exactly.
    const DURATIONS_MS: [u32; 3] = [100, 150, 200];

    fn frame(index: u32) -> Vec<u8> {
        (0..W * H)
            .flat_map(|n| {
                [
                    (n * 13 + index * 70) as u8,
                    (index * 90) as u8,
                    (n % W * 15) as u8,
                    255,
                ]
            })
            .collect()
    }

    /// Frame `index` with one-bit alpha: every third pixel transparent, with
    /// zero RGB under it.
    fn frame_with_alpha(index: u32) -> Vec<u8> {
        (0..W * H)
            .flat_map(|n| {
                if (n + index).is_multiple_of(3) {
                    [0, 0, 0, 0]
                } else {
                    [
                        (n * 13 + index * 70) as u8,
                        (index * 90) as u8,
                        (n % W * 15) as u8,
                        255,
                    ]
                }
            })
            .collect()
    }

    fn encode(format: ImageFormat, durations: &[u32]) -> Vec<u8> {
        encode_frames(format, durations, frame, false)
    }

    fn encode_frames(
        format: ImageFormat,
        durations: &[u32],
        frame: fn(u32) -> Vec<u8>,
        lossless: bool,
    ) -> Vec<u8> {
        let stride = W as usize * 4;
        let mut encoder = EncodeRequest::new(format)
            .with_lossless(lossless)
            .animation_frame_encoder(W, H)
            .unwrap();
        for (index, ms) in durations.iter().enumerate() {
            let bytes = frame(index as u32);
            let pixels =
                zenpixels::PixelSlice::new(&bytes, W, H, stride, PixelDescriptor::RGBA8_SRGB)
                    .unwrap();
            encoder.push_frame(pixels, *ms, None).unwrap();
        }
        encoder.finish(None).unwrap().into_vec()
    }

    /// Each decoded frame's exact duration in milliseconds, and its pixels.
    fn decoded(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
        let mut decoder = DecodeRequest::new(bytes).animation_frame_decoder().unwrap();
        let mut frames = Vec::new();
        while let Some(frame) = decoder.render_next_frame_owned(None).unwrap() {
            let ms = frame
                .duration()
                .ticks_at(1000)
                .expect("a whole number of milliseconds");
            frames.push((ms, rgba8(frame.into_buffer())));
        }
        frames
    }

    /// A decoded frame's pixels as RGBA8: an opaque frame rendered as RGB8 gains
    /// opaque alpha here, in the test, so frames compare in one layout.
    fn rgba8(buffer: zenpixels::PixelBuffer) -> Vec<u8> {
        let bpp = buffer.descriptor().bytes_per_pixel();
        let bytes = buffer.copy_to_contiguous_bytes();
        match bpp {
            4 => bytes,
            3 => bytes
                .chunks(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            other => panic!("unexpected {other}-byte pixels"),
        }
    }

    /// Alpha everywhere, and RGB wherever a pixel is visible (an encoder may
    /// rewrite RGB under alpha 0).
    fn assert_same_visible(actual: &[u8], expected: &[u8], what: &str) {
        assert_eq!(actual.len(), expected.len(), "{what}: size");
        for (i, (a, e)) in actual.chunks(4).zip(expected.chunks(4)).enumerate() {
            assert_eq!(a[3], e[3], "{what}: alpha of pixel {i}");
            if e[3] != 0 {
                assert_eq!(a[..3], e[..3], "{what}: RGB of pixel {i}");
            }
        }
    }

    /// Every frame, in order, with its duration; no frame repeats its predecessor.
    fn assert_every_frame(bytes: &[u8]) {
        let frames = decoded(bytes);
        let durations: Vec<u64> = frames.iter().map(|f| f.0).collect();
        let expected: Vec<u64> = DURATIONS_MS.iter().map(|&ms| u64::from(ms)).collect();
        assert_eq!(durations, expected, "every frame and its duration survive");
        for i in 1..frames.len() {
            assert_ne!(
                frames[i - 1].1,
                frames[i].1,
                "frame {i} repeats frame {}",
                i - 1
            );
        }
    }

    fn job(input: Vec<u8>, extension: &str) -> Vec<u8> {
        let result = ImageJob::new()
            .add_input(0, input)
            .add_output(1)
            .with_output_extension(extension)
            .run()
            .expect("animated job");
        result.encode_results[0].bytes.clone()
    }

    fn lossless_job(input: Vec<u8>, extension: &str) -> Vec<u8> {
        let result = ImageJob::new()
            .add_input(0, input)
            .add_output(1)
            .with_output_extension(extension)
            .with_lossless(true)
            .run()
            .expect("lossless animated job");
        result.encode_results[0].bytes.clone()
    }

    /// Lossless WebP -> lossless WebP: RGBA frames pass the route unconverted
    /// and keep every frame, its exact duration, its alpha everywhere and its
    /// RGB wherever a pixel is visible. This is visible-pixel and alpha
    /// preservation, not bit-exactness of every sample: RGB under alpha 0 is
    /// not compared (lossless WebP may rewrite it).
    #[test]
    fn imagejob_lossless_webp_keeps_visible_pixels_alpha_and_durations() {
        let input = encode_frames(ImageFormat::WebP, &DURATIONS_MS, frame_with_alpha, true);
        let frames = decoded(&lossless_job(input, "webp"));
        let durations: Vec<u64> = frames.iter().map(|f| f.0).collect();
        let expected: Vec<u64> = DURATIONS_MS.iter().map(|&ms| u64::from(ms)).collect();
        assert_eq!(durations, expected);
        for (i, (_, pixels)) in frames.iter().enumerate() {
            assert_same_visible(pixels, &frame_with_alpha(i as u32), &format!("frame {i}"));
        }
    }

    /// An opaque lossy animation renders as RGB8 (run 20). The route converts
    /// those frames to RGBA8 for the encoder: with lossless output, every output
    /// frame equals the decoded input frame with alpha 255, byte for byte, with
    /// the same duration.
    #[test]
    fn imagejob_rgb8_frames_convert_exactly_with_opaque_alpha() {
        let input = encode(ImageFormat::WebP, &DURATIONS_MS);
        let first = DecodeRequest::new(&input)
            .animation_frame_decoder()
            .unwrap()
            .render_next_frame_owned(None)
            .unwrap()
            .expect("a first frame");
        assert_eq!(
            first.pixels().descriptor().bytes_per_pixel(),
            3,
            "this control needs a decoder that renders RGB8 frames"
        );
        let source = decoded(&input);
        let output = decoded(&lossless_job(input, "webp"));
        assert_eq!(output.len(), source.len(), "frame count");
        for (i, (out, src)) in output.iter().zip(&source).enumerate() {
            assert_eq!(out.0, src.0, "frame {i} duration");
            assert_eq!(out.1, src.1, "frame {i} pixels");
        }
    }

    /// Transparent WebP -> GIF keeps every frame's exact alpha mask.
    #[test]
    fn imagejob_transparent_webp_to_gif_keeps_the_alpha_mask() {
        let input = encode_frames(ImageFormat::WebP, &DURATIONS_MS, frame_with_alpha, true);
        let frames = decoded(&job(input, "gif"));
        assert_eq!(frames.len(), DURATIONS_MS.len());
        for (i, (_, pixels)) in frames.iter().enumerate() {
            let alpha: Vec<u8> = pixels.chunks(4).map(|p| p[3]).collect();
            let expected: Vec<u8> = frame_with_alpha(i as u32).chunks(4).map(|p| p[3]).collect();
            assert_eq!(alpha, expected, "frame {i}");
        }
    }

    #[test]
    fn webp_animation_keeps_every_differing_frame_and_duration() {
        assert_every_frame(&encode(ImageFormat::WebP, &DURATIONS_MS));
    }

    #[test]
    fn a_one_frame_webp_animation_still_decodes_its_frame() {
        assert_eq!(decoded(&encode(ImageFormat::WebP, &[100])).len(), 1);
    }

    #[test]
    fn imagejob_webp_to_webp_keeps_every_frame_and_duration() {
        assert_every_frame(&job(encode(ImageFormat::WebP, &DURATIONS_MS), "webp"));
    }

    #[test]
    fn imagejob_webp_to_gif_keeps_every_frame_and_duration() {
        assert_every_frame(&job(encode(ImageFormat::WebP, &DURATIONS_MS), "gif"));
    }

    #[test]
    fn imagejob_gif_to_webp_keeps_every_frame_and_duration() {
        assert_every_frame(&job(encode(ImageFormat::Gif, &DURATIONS_MS), "webp"));
    }
}

mod pulse_forwarding {
    use super::*;
    use std::any::Any;
    use std::sync::{Arc, Mutex};
    use zencodec::Pulse;
    use zencodec::decode::{BoxedError, DynAnimationFrameDecoder};
    use zencodec::encode::{DynAnimationFrameEncoder, EncodeOutput};

    type Calls = Arc<Mutex<Vec<(&'static str, Option<usize>)>>>;
    fn identity(pulse: Option<&dyn Pulse>) -> Option<usize> {
        pulse.map(|p| p as *const dyn Pulse as *const () as usize)
    }
    struct Decoder(Box<dyn DynAnimationFrameDecoder>, Calls);
    impl DynAnimationFrameDecoder for Decoder {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn into_any(self: Box<Self>) -> Box<dyn Any> {
            self
        }
        fn info(&self) -> &zencodec::ImageInfo {
            self.0.info()
        }
        fn frame_count(&self) -> Option<u32> {
            self.0.frame_count()
        }
        fn loop_count(&self) -> Option<u32> {
            self.0.loop_count()
        }
        fn render_next_frame_owned(
            &mut self,
            pulse: Option<&dyn Pulse>,
        ) -> Result<Option<zencodec::OwnedAnimationFrame>, BoxedError> {
            self.1.lock().unwrap().push(("decode", identity(pulse)));
            self.0.render_next_frame_owned(pulse)
        }
        fn render_next_frame_to_sink(
            &mut self,
            pulse: Option<&dyn Pulse>,
            sink: &mut dyn zencodec::DecodeRowSink,
        ) -> Result<Option<zencodec::OutputInfo>, BoxedError> {
            self.1
                .lock()
                .unwrap()
                .push(("decode_sink", identity(pulse)));
            self.0.render_next_frame_to_sink(pulse, sink)
        }
    }
    struct Encoder(Box<dyn DynAnimationFrameEncoder>, Calls);
    impl DynAnimationFrameEncoder for Encoder {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn into_any(self: Box<Self>) -> Box<dyn Any> {
            self
        }
        fn push_frame(
            &mut self,
            pixels: zenpixels::PixelSlice<'_>,
            duration_ms: u32,
            pulse: Option<&dyn Pulse>,
        ) -> Result<(), BoxedError> {
            self.1.lock().unwrap().push(("encode", identity(pulse)));
            self.0.push_frame(pixels, duration_ms, pulse)
        }
        fn finish(self: Box<Self>, pulse: Option<&dyn Pulse>) -> Result<EncodeOutput, BoxedError> {
            self.1.lock().unwrap().push(("finish", identity(pulse)));
            self.0.finish(pulse)
        }
    }

    #[test]
    fn optional_pulse_reaches_native_frames_without_losing_identity_or_bytes() {
        let input = build_test_gif(3, 8, 8);
        let mut baseline = None;
        let pulse = zencodec::how_far::StopOnly::new(enough::Unstoppable);
        for control in [None, Some(&pulse as &dyn Pulse)] {
            let calls = Calls::default();
            let output = zenpipe::animation::transcode_with_pulse_and_limits(
                Box::new(Decoder(make_gif_decoder(input.clone()), calls.clone())),
                Box::new(Encoder(make_gif_encoder(8, 8), calls.clone())),
                8,
                8,
                format::RGBA8_SRGB,
                |source, _| Ok(source),
                control,
                Some(3),
            )
            .unwrap()
            .into_vec();
            let recorded = calls.lock().unwrap();
            assert_eq!(
                recorded
                    .iter()
                    .filter(|(kind, _)| *kind == "decode")
                    .count(),
                4
            );
            assert_eq!(
                recorded
                    .iter()
                    .filter(|(kind, _)| *kind == "encode")
                    .count(),
                3
            );
            assert_eq!(
                recorded
                    .iter()
                    .filter(|(kind, _)| *kind == "finish")
                    .count(),
                1
            );
            assert!(
                recorded
                    .iter()
                    .all(|(_, actual)| *actual == identity(control))
            );
            if let Some(expected) = &baseline {
                assert_eq!(&output, expected);
            } else {
                baseline = Some(output.clone());
            }
            // Read every emitted native frame, including duration and exact solid samples.
            let mut verify = make_gif_decoder(output);
            for i in 0..3 {
                let frame = verify.render_next_frame_owned(None).unwrap().unwrap();
                assert_eq!(frame.duration_ms(), 100);
                assert_pixel_gray(frame.pixels().as_strided_bytes(), i, 3);
            }
            assert!(verify.render_next_frame_owned(None).unwrap().is_none());
        }
    }

    #[test]
    fn stopped_animation_never_enters_native_frame_operations() {
        struct Stopped;
        impl enough::Stop for Stopped {
            fn check(&self) -> Result<(), enough::StopReason> {
                Err(enough::StopReason::Cancelled)
            }
        }
        let calls = Calls::default();
        let pulse = zencodec::how_far::StopOnly::new(Stopped);
        let result = zenpipe::animation::transcode_with_pulse(
            Box::new(Decoder(
                make_gif_decoder(build_test_gif(1, 8, 8)),
                calls.clone(),
            )),
            Box::new(Encoder(make_gif_encoder(8, 8), calls.clone())),
            8,
            8,
            format::RGBA8_SRGB,
            |_, _| panic!("cancelled animation constructed a pipeline"),
            Some(&pulse),
        );
        assert!(result.is_err_and(|e| matches!(e.error(), zenpipe::PipeError::Cancelled)));
        assert!(calls.lock().unwrap().is_empty());
    }
}
