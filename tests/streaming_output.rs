//! `EncoderSink` hands encoded bytes to a writer while the pipeline runs,
//! after `begin`, for an encoder whose `OutputTiming` streams (zenjpeg with
//! fixed tables): the writer sees bytes before the last strip, and the bytes
//! equal the plain sink's output. An encoder that needs the whole image
//! (zenjpeg's default, optimized tables) writes once, at `finish`.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use zencodec::decode::{DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, EncoderConfig, OutputTiming};
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig as NativeConfig, PixelLayout};
use zenjpeg::{JpegDecoderConfig, JpegEncoderConfig};
use zenpipe::codec::{DecoderSource, EncoderSink};
use zenpipe::{Sink, Source, execute, format};
use zenpixels::PixelDescriptor;

const W: u32 = 320;
const H: u32 = 240;

/// A baseline JPEG to decode from.
fn source_jpeg() -> Vec<u8> {
    let stride = W as usize * 4;
    let mut px = vec![0u8; stride * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let p = y * stride + x * 4;
            px[p..p + 4].copy_from_slice(&[(x * 3) as u8, (y * 5) as u8, ((x ^ y) * 7) as u8, 255]);
        }
    }
    let mut enc = NativeConfig::ycbcr(90.0, ChromaSubsampling::Quarter)
        .request()
        .encode_from_bytes(W, H, PixelLayout::Rgba8Srgb)
        .unwrap();
    enc.push(&px, H as usize, stride, zencodec::Unstoppable)
        .unwrap();
    enc.finish().unwrap()
}

fn decoder(jpeg: &[u8]) -> DecoderSource<'_> {
    let streaming = JpegDecoderConfig::default()
        .job()
        .dyn_streaming_decoder(Cow::Borrowed(jpeg), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();
    DecoderSource::new(streaming).unwrap()
}

fn sink(cfg: JpegEncoderConfig) -> EncoderSink<'static> {
    EncoderSink::new(cfg.job().dyn_encoder().unwrap(), format::RGBA8_SRGB)
}

/// Run the pipeline with a writer after `begin`. Returns the timing, every
/// write (in order) and how many happened before `finish`.
fn streamed(jpeg: &[u8], cfg: JpegEncoderConfig) -> (OutputTiming, Vec<Vec<u8>>, usize) {
    let writes: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let w = writes.clone();
    let mut source = decoder(jpeg);
    let mut sink = sink(cfg).with_writer(move |b| {
        w.lock().unwrap().push(b.to_vec());
        Ok(())
    });
    let timing = sink.begin(source.width(), source.height()).unwrap();
    while let Some(strip) = source.next().unwrap() {
        sink.consume(&strip).unwrap();
    }
    let before_finish = writes.lock().unwrap().len();
    sink.finish().unwrap();
    assert!(
        sink.take_output().unwrap().is_empty(),
        "the writer took the bytes"
    );
    let writes = writes.lock().unwrap().clone();
    (timing, writes, before_finish)
}

fn plain(jpeg: &[u8], cfg: JpegEncoderConfig) -> Vec<u8> {
    let mut source = decoder(jpeg);
    let mut sink = sink(cfg);
    execute(&mut source, &mut sink).unwrap();
    sink.take_output().unwrap().into_vec()
}

#[test]
fn fixed_table_jpeg_streams_through_the_writer() {
    let jpeg = source_jpeg();
    let cfg = JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .with_progressive(false)
        .with_huffman(false);
    let (timing, writes, before_finish) = streamed(&jpeg, cfg.clone());
    assert!(timing.streams(), "{timing:?}");
    assert!(
        before_finish > 1,
        "header and rows before finish: {before_finish} writes"
    );
    assert_eq!(writes.concat(), plain(&jpeg, cfg));
}

#[test]
fn optimized_tables_write_once_at_finish() {
    let jpeg = source_jpeg();
    let cfg = JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter);
    let (timing, writes, before_finish) = streamed(&jpeg, cfg.clone());
    assert_eq!(timing, OutputTiming::AT_FINISH);
    assert_eq!(before_finish, 0);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes.concat(), plain(&jpeg, cfg));
}

#[test]
fn take_pending_output_hands_over_the_bytes_without_a_writer() {
    let jpeg = source_jpeg();
    let cfg = JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .with_progressive(false)
        .with_huffman(false);
    let mut source = decoder(&jpeg);
    let mut sink = sink(cfg.clone());
    assert!(
        sink.begin(source.width(), source.height())
            .unwrap()
            .streams()
    );
    let mut out = Vec::new();
    let mut chunks = 0;
    while let Some(strip) = source.next().unwrap() {
        sink.consume(&strip).unwrap();
        let mut chunk = Vec::new();
        sink.take_pending_output(&mut chunk);
        assert!(sink.pending_output().is_empty());
        chunks += usize::from(!chunk.is_empty());
        out.extend_from_slice(&chunk);
    }
    sink.finish().unwrap();
    out.extend_from_slice(sink.take_output().unwrap().data());
    assert!(chunks > 1, "{chunks} chunks before finish");
    assert_eq!(out, plain(&jpeg, cfg));
}
