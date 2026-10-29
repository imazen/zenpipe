//! A complete animation has the same bytes with or without incremental drains.
#![cfg(all(feature = "gif", feature = "webp", feature = "png"))]

use zencodec::encode::{AnimationOutputTiming, DynAnimationFrameEncoder};
use zencodecs::{EncodeRequest, ImageFormat};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 19;
const H: u32 = 23;

fn drain(encoder: &mut dyn DynAnimationFrameEncoder, output: &mut Vec<u8>, mode: u8) {
    match mode {
        1 => loop {
            let n = encoder.pending_output().len().min(7);
            if n == 0 {
                break;
            }
            output.extend_from_slice(&encoder.pending_output()[..n]);
            encoder.consume_output(n);
        },
        2 => {
            let mut taken = vec![0xC7];
            encoder.take_output(&mut taken);
            assert_eq!(taken[0], 0xC7);
            output.extend_from_slice(&taken[1..]);
            let mut empty = Vec::new();
            encoder.take_output(&mut empty);
            assert!(empty.is_empty());
        }
        _ => {}
    }
}

fn encode(
    format: ImageFormat,
    begin: bool,
    mode: u8,
    shared_frames: usize,
) -> (Vec<u8>, Option<AnimationOutputTiming>) {
    let config = zencodecs::config::CodecConfig::default().with_gif_encoder(
        zengif::GifEncoderConfig::new()
            .with_max_buffer_frames(shared_frames)
            .inner()
            .clone(),
    );
    let mut encoder = EncodeRequest::new(format)
        .with_quality(85.0)
        .with_codec_config(&config)
        .animation_frame_encoder(W, H)
        .unwrap();
    let timing = begin.then(|| {
        encoder
            .begin(W, H, PixelDescriptor::RGBA8_SRGB, Some(3))
            .unwrap()
    });
    let mut output = Vec::new();
    let mut early = false;
    drain(encoder.as_mut(), &mut output, mode);
    for frame in 0..3 {
        let stride = (W as usize + 3) * 4;
        let mut data = vec![0xCD; stride * H as usize];
        for y in 0..H as usize {
            for x in 0..W as usize {
                let offset = y * stride + x * 4;
                data[offset..offset + 4].copy_from_slice(&[
                    (x * 11 + frame * 31) as u8,
                    (y * 7 + frame * 19) as u8,
                    (x * y + frame * 43) as u8,
                    if (x + y + frame) % 5 == 0 { 0 } else { 255 },
                ]);
            }
        }
        let pixels = PixelSlice::new(&data, W, H, stride, PixelDescriptor::RGBA8_SRGB).unwrap();
        encoder
            .push_frame(pixels, 40 + frame as u32 * 10, None)
            .unwrap();
        if timing.is_some_and(|t| !t.streams()) {
            assert!(encoder.pending_output().is_empty());
        }
        drain(encoder.as_mut(), &mut output, mode);
        early |= frame < 2 && !output.is_empty();
    }
    if mode != 0 {
        if timing.is_some_and(|t| t.streams()) {
            assert!(early, "{format:?} claimed streaming but held all frames");
        }
        encoder.consume_output(usize::MAX);
    }
    output.extend_from_slice(encoder.finish(None).unwrap().data());
    (output, timing)
}

#[test]
fn animation_drains_preserve_the_complete_file() {
    let mut streaming = false;
    let mut at_finish = false;
    for (format, shared_frames) in [
        (ImageFormat::Gif, 2),
        (ImageFormat::Gif, 8),
        (ImageFormat::WebP, 8),
        (ImageFormat::Png, 8),
    ] {
        let (whole, _) = encode(format, false, 0, shared_frames);
        let (undrained, timing) = encode(format, true, 0, shared_frames);
        assert_eq!(undrained, whole, "{format:?} declaration changed the file");
        let timing = timing.unwrap();
        streaming |= timing.streams();
        at_finish |= !timing.streams();
        for mode in [1, 2] {
            let (streamed, got_timing) = encode(format, true, mode, shared_frames);
            assert_eq!(got_timing, Some(timing));
            assert_eq!(streamed, whole, "{format:?} drain mode {mode}");
        }
        println!("animation {format:?}: {timing:?}");
    }
    assert!(streaming, "no streaming animation was exercised");
    assert!(at_finish, "no buffered animation was exercised");
}
