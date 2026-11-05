//! A resize after a filter must not change what the filter produced, beyond resampling.
#![cfg(all(feature = "job", feature = "nodes-png", feature = "nodes-filters"))]
use zencodec::ImageFormat;
use zencodecs::{BoolKeep, CodecIntent, DecodeRequest, EncodeRequest, FormatChoice};
use zenpipe::bridge::{NodeConverter, ZenFiltersConverter};
use zenpipe::job::{CmsMode, ImageJob};
use zenpixels::{PixelBuffer, PixelDescriptor as D};

fn solid_png() -> Vec<u8> {
    let mut b = PixelBuffer::try_new(16, 16, D::RGBA8_SRGB).unwrap();
    let mut m = b.as_slice_mut();
    for y in 0..16 {
        for c in m.row_mut(y).as_chunks_mut::<4>().0 {
            *c = [200, 100, 50, 255];
        }
    }
    EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .encode(b.as_slice(), true)
        .unwrap()
        .into_vec()
}

/// Largest and smallest value per channel of a job's output.
fn run(input: &[u8], query: &str) -> ([u8; 3], [u8; 3]) {
    let nodes = zenpipe::full_registry().from_querystring(query).instances;
    let converters: Vec<&dyn NodeConverter> = vec![&ZenFiltersConverter];
    let result = ImageJob::new()
        .add_input(0, input.to_vec())
        .add_output(1)
        .with_cms(CmsMode::Preserve)
        .with_nodes(&nodes)
        .with_converters(&converters)
        .with_intent(CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Png)),
            lossless: Some(BoolKeep::True),
            ..Default::default()
        })
        .run()
        .unwrap();
    let decoded = DecodeRequest::new(&result.encode_results[0].bytes)
        .decode_full_frame()
        .unwrap();
    let pixels = decoded.pixels();
    let bytes = pixels.descriptor().bytes_per_pixel();
    let (mut low, mut high) = ([255u8; 3], [0u8; 3]);
    for y in 0..pixels.rows() {
        for px in pixels.row(y).chunks_exact(bytes) {
            for c in 0..3 {
                low[c] = low[c].min(px[c]);
                high[c] = high[c].max(px[c]);
            }
        }
    }
    (low, high)
}

#[test]
fn resizing_a_filtered_image_keeps_its_transfer() {
    let input = solid_png();
    for (query, want) in [
        ("w=8", [200, 100, 50]),
        ("s.saturation=1", [200, 100, 50]),
        ("w=8&s.saturation=1", [200, 100, 50]),
        ("w=16&s.saturation=1", [200, 100, 50]),
        ("h=8&s.brightness=0", [200, 100, 50]),
    ] {
        let (low, high) = run(&input, query);
        for c in 0..3 {
            assert!(
                low[c].abs_diff(want[c]) <= 2 && high[c].abs_diff(want[c]) <= 2,
                "{query}: {low:?}..{high:?}, want {want:?}"
            );
        }
    }
    // The filter's own result, with and without the resize.
    let alone = run(&input, "s.saturation=0.2").0;
    let resized = run(&input, "w=8&s.saturation=0.2").0;
    for c in 0..3 {
        assert!(
            alone[c].abs_diff(resized[c]) <= 2,
            "{alone:?} vs {resized:?}"
        );
    }
}
