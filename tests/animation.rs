//! Tests for animation support — frame-by-frame decode, process, encode.
//!
//! Uses zengif for end-to-end testing: generate animated GIF → decode
//! frame-by-frame → process through pipeline → encode back to GIF.

use std::borrow::Cow;

use hashbrown::HashMap;
use zencodec::decode::{DecodeJob as _, DecoderConfig as _, FullFrameDecoder as _};
use zencodec::encode::{EncodeJob as _, EncoderConfig as _, FullFrameEncoder as _};
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
        .full_frame_encoder()
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

fn make_gif_decoder(data: Vec<u8>) -> Box<dyn zencodec::decode::DynFullFrameDecoder + Send> {
    let dec = GifDecoderConfig::new();
    let frame_dec = dec
        .job()
        .full_frame_decoder(Cow::Owned(data), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();
    Box::new(SendFullFrameDecoderShim(frame_dec))
}

fn make_gif_encoder(
    width: u16,
    height: u16,
) -> Box<dyn zencodec::encode::DynFullFrameEncoder + Send> {
    let config = GifEncoderConfig::new();
    let encoder = config
        .job()
        .with_canvas_size(width as u32, height as u32)
        .full_frame_encoder()
        .unwrap();
    Box::new(SendFullFrameEncoderShim(encoder))
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

    drain(&mut source);

    assert!(source.advance_frame().unwrap());
    let info = source.frame_info().unwrap();
    assert_eq!(info.index, 1);
    drain(&mut source);

    assert!(source.advance_frame().unwrap());
    let info = source.frame_info().unwrap();
    assert_eq!(info.index, 2);
    drain(&mut source);

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
        .full_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    assert!(verify.render_next_frame_owned(None).unwrap().is_some());
    assert!(verify.render_next_frame_owned(None).unwrap().is_some());
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
        .full_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    for i in 0..3 {
        assert!(
            verify.render_next_frame_owned(None).unwrap().is_some(),
            "missing frame {i}"
        );
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
        .full_frame_decoder(Cow::Owned(encoded), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();

    assert_eq!(verify.info().width, 4);
    assert_eq!(verify.info().height, 4);
    assert!(verify.render_next_frame_owned(None).unwrap().is_some());
    assert!(verify.render_next_frame_owned(None).unwrap().is_some());
    assert!(verify.render_next_frame_owned(None).unwrap().is_none());
}

// =========================================================================
// Helpers
// =========================================================================

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

// =========================================================================
// Send shims
// =========================================================================

struct SendFullFrameDecoderShim<D>(D);
unsafe impl<D: Send> Send for SendFullFrameDecoderShim<D> {}

impl<D: zencodec::decode::FullFrameDecoder + Send + 'static> zencodec::decode::DynFullFrameDecoder
    for SendFullFrameDecoderShim<D>
{
    fn as_any(&self) -> &dyn std::any::Any {
        &self.0
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        &mut self.0
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        Box::new(self.0)
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
        stop: Option<&dyn enough::Stop>,
    ) -> Result<Option<zencodec::OwnedFullFrame>, zencodec::encode::BoxedError> {
        self.0
            .render_next_frame_owned(stop)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
    fn render_next_frame_to_sink(
        &mut self,
        stop: Option<&dyn enough::Stop>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<Option<zencodec::decode::OutputInfo>, zencodec::encode::BoxedError> {
        self.0
            .render_next_frame_to_sink(stop, sink)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
}

struct SendFullFrameEncoderShim<E>(E);
unsafe impl<E: Send> Send for SendFullFrameEncoderShim<E> {}

impl<E: zencodec::encode::FullFrameEncoder + Send + 'static> zencodec::encode::DynFullFrameEncoder
    for SendFullFrameEncoderShim<E>
{
    fn as_any(&self) -> &dyn std::any::Any {
        &self.0
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        &mut self.0
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        Box::new(self.0)
    }
    fn push_frame(
        &mut self,
        pixels: zenpixels::PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn enough::Stop>,
    ) -> Result<(), zencodec::encode::BoxedError> {
        self.0
            .push_frame(pixels, duration_ms, stop)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
    fn finish(
        self: Box<Self>,
        stop: Option<&dyn enough::Stop>,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .finish(stop)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
}
