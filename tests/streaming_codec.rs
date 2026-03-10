//! Integration test: streaming 4K JPEG through decode → encode via zenjpeg.
//!
//! Validates that the streaming pipeline processes a 4K image strip-by-strip
//! without materializing the full image. Run under heaptrack to verify peak
//! heap stays bounded.
//!
//! Run: `cargo test --features codec --test streaming_codec -- --nocapture`
//! Heaptrack: `heaptrack cargo test --features codec --test streaming_codec -- --nocapture`
#![cfg(feature = "codec")]

use std::borrow::Cow;

use zencodec::decode::{DecodeJob, DecoderConfig, StreamingDecode};
use zenjpeg::JpegDecoderConfig;
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
use zenpixels::PixelDescriptor;

const WIDTH: u32 = 3840;
const HEIGHT: u32 = 2160;

/// Generate a 4K gradient JPEG, streaming strip-by-strip into the encoder.
fn generate_4k_jpeg() -> Vec<u8> {
    let bpp = 4usize; // RGBA8
    let stride = WIDTH as usize * bpp;

    // Use baseline (non-progressive) to avoid progressive token buffer overhead.
    let config = EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter).progressive(false);
    let mut enc = config
        .request()
        .encode_from_bytes(WIDTH, HEIGHT, PixelLayout::Rgba8Srgb)
        .expect("encoder creation");

    let strip_h = 16u32;
    let mut strip_buf = vec![0u8; stride * strip_h as usize];

    let mut y = 0u32;
    while y < HEIGHT {
        let rows = strip_h.min(HEIGHT - y);
        for r in 0..rows {
            let row_start = r as usize * stride;
            for x in 0..WIDTH {
                let px = row_start + x as usize * bpp;
                strip_buf[px] = (x * 255 / WIDTH) as u8;
                strip_buf[px + 1] = ((y + r) * 255 / HEIGHT) as u8;
                strip_buf[px + 2] = 128;
                strip_buf[px + 3] = 255;
            }
        }
        enc.push(
            &strip_buf[..rows as usize * stride],
            rows as usize,
            stride,
            zencodec::Unstoppable,
        )
        .expect("push rows");
        y += rows;
    }

    enc.finish().expect("finish encode")
}

#[test]
fn stream_4k_jpeg_decode_encode() {
    // --- Generate test data ---
    let jpeg_data = generate_4k_jpeg();
    eprintln!("Generated 4K JPEG: {} KB", jpeg_data.len() / 1024);

    // --- Streaming decode → encode ---
    let dec_config = JpegDecoderConfig::default();
    let job = dec_config.job();
    let mut decoder = job
        .streaming_decoder(Cow::Borrowed(&jpeg_data), &[PixelDescriptor::RGBA8_SRGB])
        .expect("streaming decoder creation");

    let info = decoder.info().clone();
    assert_eq!(info.width, WIDTH);
    assert_eq!(info.height, HEIGHT);

    // Native streaming encoder (truly row-streaming, no accumulation).
    // Baseline mode avoids the progressive token buffer that stores all coefficients.
    let enc_config = EncoderConfig::ycbcr(80.0, ChromaSubsampling::Quarter).progressive(false);
    let mut encoder = enc_config
        .request()
        .encode_from_bytes(WIDTH, HEIGHT, PixelLayout::Rgba8Srgb)
        .expect("encoder creation");

    let mut total_rows: u32 = 0;
    let mut strip_count: u32 = 0;

    while let Some((_y, pixels)) = decoder.next_batch().expect("next_batch") {
        let rows: u32 = pixels.rows();
        let data = pixels.as_strided_bytes();
        let stride_bytes = pixels.stride();

        encoder
            .push(data, rows as usize, stride_bytes, zencodec::Unstoppable)
            .expect("encoder push");

        total_rows += rows;
        strip_count += 1;
    }

    assert_eq!(total_rows, HEIGHT, "didn't decode all rows");
    assert!(
        strip_count > 1,
        "expected multiple strips, got {strip_count}"
    );

    let output = encoder.finish().expect("encoder finish");
    assert!(
        output.len() > 1000,
        "output too small: {} bytes",
        output.len()
    );

    // Verify output is decodable
    let verify_config = JpegDecoderConfig::default();
    let verify_job = verify_config.job();
    let verify_info = verify_job
        .probe(&output)
        .expect("output should be valid JPEG");
    assert_eq!(verify_info.width, WIDTH);
    assert_eq!(verify_info.height, HEIGHT);

    eprintln!(
        "Streaming 4K: {} strips, input {}KB → output {}KB",
        strip_count,
        jpeg_data.len() / 1024,
        output.len() / 1024
    );
}

/// Test the zenpipe codec bridge types (DecoderSource / EncoderSink)
/// with a smaller image to verify the API works.
///
/// Note: EncoderSink uses zencodec's DynEncoder which accumulates all rows
/// before encoding on finish(). This is a zencodec adapter limitation —
/// zenjpeg's native BytesEncoder streams truly.
#[test]
fn codec_bridge_roundtrip() {
    use zenpipe::codec::{DecoderSource, EncoderSink};
    use zenpipe::{PixelFormat, Source, execute};

    // Small image to test the bridge (256×256)
    let w = 256u32;
    let h = 256u32;
    let bpp = 4usize;
    let stride = w as usize * bpp;

    // Generate test JPEG
    let config = EncoderConfig::ycbcr(90.0, ChromaSubsampling::Quarter);
    let mut enc = config
        .request()
        .encode_from_bytes(w, h, PixelLayout::Rgba8Srgb)
        .expect("encoder creation");

    let mut buf = vec![0u8; stride * h as usize];
    for y in 0..h {
        for x in 0..w {
            let px = (y as usize * stride) + (x as usize * bpp);
            buf[px] = (x & 0xFF) as u8;
            buf[px + 1] = (y & 0xFF) as u8;
            buf[px + 2] = 100;
            buf[px + 3] = 255;
        }
    }
    enc.push(&buf, h as usize, stride, zencodec::Unstoppable)
        .expect("push");
    let jpeg_data = enc.finish().expect("finish");

    // Decode via zencodec dyn → DecoderSource
    let dec_config = JpegDecoderConfig::default();
    let job = dec_config.job();
    let streaming: Box<dyn zencodec::decode::DynStreamingDecoder + Send> = {
        // Use the concrete streaming decoder and wrap it manually,
        // since dyn_streaming_decoder() doesn't return + Send.
        let concrete = job
            .streaming_decoder(Cow::Borrowed(&jpeg_data), &[PixelDescriptor::RGBA8_SRGB])
            .expect("streaming decoder");
        Box::new(SendStreamingDecoderShim(concrete))
    };

    let mut source =
        DecoderSource::new(streaming, PixelFormat::Rgba8).expect("DecoderSource creation");
    assert_eq!(source.width(), w);
    assert_eq!(source.height(), h);

    // Encode via zencodec dyn → EncoderSink
    let enc_config2 = zenjpeg::JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter);
    let enc_job =
        <zenjpeg::JpegEncoderConfig as zencodec::encode::EncoderConfig>::job(&enc_config2);
    let dyn_enc: Box<dyn zencodec::encode::DynEncoder + Send> = {
        let concrete = zencodec::encode::EncodeJob::encoder(enc_job).expect("encoder");
        Box::new(SendEncoderShim(concrete))
    };

    let mut sink = EncoderSink::new(dyn_enc, PixelFormat::Rgba8);

    // Execute pipeline
    execute(&mut source, &mut sink).expect("pipeline execution");

    let output = sink.take_output().expect("encoder output");
    let out_bytes = output.into_vec();
    assert!(out_bytes.len() > 100, "output too small");

    // Verify
    let verify_config = JpegDecoderConfig::default();
    let verify_info = verify_config.job().probe(&out_bytes).expect("valid JPEG");
    assert_eq!(verify_info.width, w);
    assert_eq!(verify_info.height, h);

    eprintln!(
        "Codec bridge roundtrip: {}×{}, {} KB",
        w,
        h,
        out_bytes.len() / 1024
    );
}

// ============================================================================
// Send shims — zencodec's dyn dispatch doesn't propagate Send bounds,
// but the concrete zenjpeg types are Send-safe. These thin wrappers
// bridge the gap so the types work with zenpipe's Source/Sink (which
// require Send).
// ============================================================================

struct SendStreamingDecoderShim<S>(S);

impl<S: zencodec::decode::StreamingDecode + Send> zencodec::decode::DynStreamingDecoder
    for SendStreamingDecoderShim<S>
{
    fn next_batch(
        &mut self,
    ) -> Result<Option<(u32, zenpixels::PixelSlice<'_>)>, zencodec::encode::BoxedError> {
        self.0
            .next_batch()
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn info(&self) -> &zencodec::ImageInfo {
        self.0.info()
    }
}

struct SendEncoderShim<E>(E);

impl<E: zencodec::encode::Encoder + Send> zencodec::encode::DynEncoder for SendEncoderShim<E> {
    fn preferred_strip_height(&self) -> u32 {
        self.0.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: zenpixels::PixelSlice<'_>,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode(pixels)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode_srgba8(data, make_opaque, width, height, stride_pixels)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn push_rows(
        &mut self,
        rows: zenpixels::PixelSlice<'_>,
    ) -> Result<(), zencodec::encode::BoxedError> {
        self.0
            .push_rows(rows)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn finish(
        self: Box<Self>,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .finish()
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn encode_from(
        self: Box<Self>,
        source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode_from(source)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
}
