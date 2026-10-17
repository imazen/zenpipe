//! `TranscodeSink` declares the image to the encoder when the decoder
//! begins, so an encoder whose `OutputTiming` streams (zenjpeg with fixed
//! tables) hands bytes to the writer while rows are still being decoded,
//! and the bytes equal the one-shot transcode.

#![cfg(all(feature = "jpeg", feature = "png"))]

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use zencodec::decode::{DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig, OutputTiming};
use zencodecs::TranscodeSink;
use zenjpeg::JpegEncoderConfig;
use zenjpeg::encoder::ChromaSubsampling;
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 200;
const H: u32 = 120;

fn rgb() -> Vec<u8> {
    (0..W * H)
        .flat_map(|i| {
            let (x, y) = (i % W, i / W);
            [(x * 3) as u8, (y * 2) as u8, (x ^ y) as u8]
        })
        .collect()
}

fn png() -> Vec<u8> {
    let px = rgb();
    let s = PixelSlice::new(&px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    zenpng::PngEncoderConfig::new()
        .job()
        .encoder()
        .unwrap()
        .encode(s)
        .unwrap()
        .into_vec()
}

fn cfg() -> JpegEncoderConfig {
    JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .with_progressive(false)
        .with_huffman(false)
}

#[test]
fn transcode_streams_while_decoding() {
    let file = png();
    let writes: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let w = writes.clone();
    let mut sink = TranscodeSink::new(
        cfg().job().dyn_encoder().unwrap(),
        JpegEncoderConfig::supported_descriptors(),
    )
    .with_writer(move |b| {
        w.lock().unwrap().push(b.to_vec());
        Ok(())
    });
    zenpng::PngDecoderConfig::new()
        .job()
        .push_decoder(
            Cow::Borrowed(&file),
            &mut sink,
            &[PixelDescriptor::RGB8_SRGB],
        )
        .unwrap();
    let timing = sink
        .output_timing()
        .expect("the decoder declared the image");
    assert!(timing.streams(), "{timing:?}");
    let before_finish = writes.lock().unwrap().len();
    assert!(before_finish > 1, "{before_finish} writes before finish");
    assert!(sink.finish_encode().unwrap().is_empty());

    let px = rgb();
    let s = PixelSlice::new(&px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let want = cfg().job().encoder().unwrap().encode(s).unwrap().into_vec();
    assert_eq!(writes.lock().unwrap().concat(), want);
}

#[test]
fn without_a_streaming_encoder_everything_arrives_at_finish() {
    let file = png();
    let mut sink = TranscodeSink::new(
        JpegEncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
            .job()
            .dyn_encoder()
            .unwrap(),
        JpegEncoderConfig::supported_descriptors(),
    );
    zenpng::PngDecoderConfig::new()
        .job()
        .push_decoder(
            Cow::Borrowed(&file),
            &mut sink,
            &[PixelDescriptor::RGB8_SRGB],
        )
        .unwrap();
    assert_eq!(sink.output_timing(), Some(OutputTiming::AT_FINISH));
    assert!(sink.pending_output().is_empty());
    assert!(!sink.finish_encode().unwrap().is_empty());
}
