//! Run the site's checked JPEG/PNG guide against this frozen graph.
#![forbid(unsafe_code)]

#[path = "../src/guide.rs"]
mod guide;

use core::future::Future;
use core::task::{Context, Poll, Waker};
use zencodec::ImageFormat;
use zencodec_io::tokio_io::AsyncSource;
use zenpixels::{PixelDescriptor, PixelSlice};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

/// An sRGB JPEG from zencodecs' own encoder.
fn jpeg() -> Vec<u8> {
    let rgb: Vec<u8> = (0..WIDTH * HEIGHT)
        .flat_map(|i| {
            let (x, y) = (i % WIDTH, i / WIDTH);
            [(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8]
        })
        .collect();
    let pixels = PixelSlice::new(
        &rgb,
        WIDTH,
        HEIGHT,
        WIDTH as usize * 3,
        PixelDescriptor::RGB8_SRGB,
    )
    .unwrap();
    zencodecs::EncodeRequest::new(ImageFormat::Jpeg)
        .encode(pixels, false)
        .unwrap()
        .into_vec()
}

/// Serves at most 31 bytes per read, so the async route crosses many reads.
struct Bytes<'a>(&'a [u8]);
impl AsyncSource for Bytes<'_> {
    async fn read_at(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let remaining = self.0.get(usize::try_from(offset)?..).unwrap_or_default();
        let n = remaining.len().min(out.len()).min(31);
        out[..n].copy_from_slice(&remaining[..n]);
        Ok(n)
    }
    async fn len(&mut self) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.0.len() as u64)
    }
    fn len_hint(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }
}

fn complete<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..100_000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
    panic!("guide future made no bounded progress");
}

#[test]
fn guide_recipes_run_on_the_frozen_graph() {
    let codecs = guide::gallery_codecs();
    let source = jpeg();
    let png = guide::derivative(&codecs, &source).unwrap();
    assert_eq!(&png.data()[..8], b"\x89PNG\r\n\x1a\n");
    for input in [source.as_slice(), png.data()] {
        let preview = guide::preview(&codecs, input).unwrap();
        assert_eq!((preview.width(), preview.height()), (WIDTH, HEIGHT));
        let request = guide::web_request();
        let expected = request.transcode(&codecs, input).unwrap();
        let conditional = guide::for_tenant(&request, &codecs, input, false).unwrap();
        assert_eq!(conditional.data(), expected.data());
        let refusal = guide::for_tenant(&request, &codecs, input, true).unwrap_err();
        assert!(zencodec::find_cause::<guide::WorkspaceEvidenceRequired>(&refusal).is_some());
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("guide-input");
        std::fs::write(&path, input).unwrap();
        let file = guide::from_file(&request, &codecs, &path).unwrap();
        assert_eq!(file.data(), expected.data());
        let network = complete(guide::from_network(&request, &codecs, &mut Bytes(input))).unwrap();
        assert_eq!(network.data(), expected.data());
    }
}

#[test]
fn guide_density_survives_the_default_transcode() {
    use zencodec::{Resolution, ResolutionUnit};
    let codecs = guide::gallery_codecs();
    let preview = guide::preview(&codecs, &jpeg()).unwrap();
    let resolution = Resolution {
        x: 254.0,
        y: 127.0,
        unit: ResolutionUnit::Inch,
    };
    let encoded = guide::with_density(&codecs, preview.pixels(), resolution).unwrap();
    let retained = guide::derivative(&codecs, encoded.data()).unwrap();
    let decoded = guide::preview(&codecs, retained.data()).unwrap();
    assert!(resolution.is_equivalent_to(&decoded.info().resolution.unwrap()));
}
