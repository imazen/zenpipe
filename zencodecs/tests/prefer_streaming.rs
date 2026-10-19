//! Generic streaming preferences preserve selected native encoder settings.
#![cfg(feature = "jpeg")]

use zencodecs::{EncodeRequest, ImageFormat, config::CodecConfig};
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
fn jpeg_preference_reaches_both_paths_and_native_settings_win() {
    let (w, h) = (67, 93);
    let data: Vec<u8> = (0..w * h * 3)
        .map(|i| ((i * 73) ^ (i >> 5)) as u8)
        .collect();
    let pixels = PixelSlice::new(&data, w, h, w as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let empty = CodecConfig::default();
    let decoder_only = CodecConfig::default().with_jpeg_decoder(Default::default());
    #[cfg(feature = "gif")]
    let other_format =
        CodecConfig::default().with_gif_encoder(zengif::GifEncoderConfig::new().inner().clone());
    let explicit = CodecConfig::default().with_jpeg_encoder(
        EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
            .progressive(true)
            .huffman(true),
    );
    let configs = [
        (None, false),
        (Some(&empty), false),
        (Some(&explicit), true),
        (Some(&decoder_only), false),
    ];
    #[cfg(feature = "gif")]
    let configs = [configs.as_slice(), &[(Some(&other_format), false)]].concat();
    for (config, has_override) in configs {
        for prefer in [false, true] {
            let make = || {
                let request = EncodeRequest::new(ImageFormat::Jpeg)
                    .with_effort(2)
                    .with_prefer_streaming(prefer);
                if let Some(config) = config {
                    request.with_codec_config(config)
                } else {
                    request
                }
            };
            let whole = make().encode(pixels.clone(), false).unwrap();
            let mut encoder = make().build_streaming_encoder(w, h).unwrap().encoder;
            let timing = encoder.begin(w, h, pixels.descriptor()).unwrap();
            assert_eq!(timing.streams(), prefer && !has_override);
            let mut out = Vec::new();
            encoder.take_output(&mut out);
            for y in (0..h).step_by(7) {
                let rows = 7.min(h - y);
                let start = y as usize * w as usize * 3;
                encoder
                    .push_rows(
                        PixelSlice::new(
                            &data[start..],
                            w,
                            rows,
                            w as usize * 3,
                            pixels.descriptor(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                encoder.take_output(&mut out);
            }
            out.extend_from_slice(encoder.finish().unwrap().data());
            assert_eq!(out, whole.data());
        }
    }
}
