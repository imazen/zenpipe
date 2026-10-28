//! Backend selection and Pulse retention must reach real AVIF dispatch.
#![cfg(feature = "avif-svt")]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zencodecs::{EncodeRequest, ImageFormat, config::CodecConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

#[derive(Debug)]
struct CountingStop(Arc<AtomicUsize>);
impl enough::Stop for CountingStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn selected_svt_backend_encodes_strided_pixels_and_retains_pulse() {
    let (w, h) = (17u32, 19u32);
    let stride = w as usize * 3 + 9;
    let pixels: Vec<u8> = (0..(h as usize - 1) * stride + w as usize * 3)
        .map(|i| (i * 37) as u8)
        .collect();
    let view = PixelSlice::new(&pixels, w, h, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let checks = Arc::new(AtomicUsize::new(0));
    let config = CodecConfig::default().with_avif_backend(zenavif::Av1Backend::Zenav1Svt);
    let output = EncodeRequest::new(ImageFormat::Avif)
        .with_codec_config(&config)
        .with_pulse(how_far::SharedPulse::new(how_far::StopOnly::new(
            CountingStop(checks.clone()),
        )))
        .with_effort(0)
        .encode(view, false)
        .unwrap();
    assert_eq!(&output.data()[4..8], b"ftyp");
    assert!(
        checks.load(Ordering::Relaxed) > 1,
        "pulse must survive into the encoder"
    );
}

#[cfg(not(feature = "avif-aom"))]
#[test]
fn unavailable_backend_is_refused_without_fallback() {
    let bytes = [128u8; 8 * 8 * 3];
    let view = PixelSlice::new(&bytes, 8, 8, 24, PixelDescriptor::RGB8_SRGB).unwrap();
    let config = CodecConfig::default().with_avif_backend(zenavif::Av1Backend::Zenav1Aom);
    assert!(
        EncodeRequest::new(ImageFormat::Avif)
            .with_codec_config(&config)
            .encode(view, false)
            .is_err()
    );
}

#[cfg(feature = "avif-decode")]
#[test]
fn native_avif_444_and_lossless_survive_dispatch_overrides() {
    let (w, h) = (17u32, 19u32);
    let stride = w as usize * 3 + 9;
    let bytes: Vec<u8> = (0..(h as usize - 1) * stride + w as usize * 3)
        .map(|i| (i * 37 + i / 7) as u8)
        .collect();
    let pixels = PixelSlice::new(&bytes, w, h, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let mut native = zenavif::AvifEncoderConfig::new().with_lossless_mode(true);
    *native.inner_mut() = native
        .inner()
        .clone()
        .backend(zenavif::Av1Backend::Zenav1Svt)
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv444)
        .color_model(zenavif::EncodeColorModel::Rgb)
        .threads(Some(1));
    let config = CodecConfig::default()
        .with_avif_encoder(native)
        .with_avif_backend(zenavif::Av1Backend::Zenav1Svt);
    let encoded = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(5.0)
        .with_effort(10)
        .with_codec_config(&config)
        .encode(pixels.clone(), false)
        .unwrap();
    let mut streamed = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(5.0)
        .with_effort(10)
        .with_codec_config(&config)
        .build_streaming_encoder(w, h)
        .unwrap()
        .encoder;
    streamed.begin(w, h, pixels.descriptor()).unwrap();
    streamed.push_rows(pixels).unwrap();
    let streamed = streamed.finish().unwrap();
    assert_eq!(
        streamed.data(),
        encoded.data(),
        "streaming must retain the same native mode and pixels"
    );
    let image = zenavif::decode(encoded.data()).unwrap();
    let image = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
    let expected: Vec<_> = bytes
        .chunks(stride)
        .take(h as usize)
        .flat_map(|row| row[..w as usize * 3].chunks_exact(3))
        .map(|p| rgb::Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    assert_eq!(
        *image.buf(),
        expected.as_slice(),
        "native 444 lossless must survive generic quality/effort and backend-only defaults"
    );
}
