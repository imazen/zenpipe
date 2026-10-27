//! Range-request cost of decoding through `zenpipe::io` over the reused IMI
//! transport model (calculated network costs, not WAN measurements).
//!
//! The controlled JPEG and PNG routes need their whole compressed input before
//! their final interpretation, so a region request cannot skip bytes. What a
//! transport policy controls for them is the number of dependent rounds:
//! sequential reads of `read_size` each wait for the previous one.

#[path = "support/transport_model.rs"]
mod transport_model;

use std::sync::{Arc, Mutex};

use transport_model::{Config, Ledger, Transport};
use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{DecodeRequest, PolicySet};
use zencodec::source::SliceReadAt;
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, ImageFormat};
use zencodec_io::DriveOptions;
use zenpipe::Source;
use zenpipe::io::{DecodeOptions, DecodeSource};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 320;
const H: u32 = 240;
/// 12.5 MB/s shared link, as in the reused study.
const BYTES_PER_MS: f64 = 12_500.0;

fn noisy() -> Vec<u8> {
    // Deterministic xorshift samples: incompressible enough to give a
    // multi-round object at small read sizes.
    let mut state = 0x9e37_79b9u32;
    (0..W * H * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect()
}

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let samples = noisy();
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    let png = zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec();
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    let jpeg = zenjpeg::JpegEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec();
    vec![("png", png), ("jpeg", jpeg)]
}

fn fixture(name: &str) -> Vec<u8> {
    fixtures()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, file)| file)
        .unwrap()
}

fn codecs() -> CodecSet {
    CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
}

fn config(rtt_ms: f64, cache_bytes: u64, shared_cache: bool) -> Config {
    Config {
        ranges: true,
        cache_bytes,
        latency_ms: rtt_ms,
        bytes_per_ms: BYTES_PER_MS,
        concurrency: 8,
        merge_gap: 4096,
        max_range: 1 << 20,
        part_size: 0,
        shared_cache,
    }
}

fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(strip) = source.next().unwrap() {
        for r in 0..strip.rows() {
            out.extend_from_slice(strip.row(r));
        }
    }
    out
}

fn decode_through(
    file: &[u8],
    cfg: Config,
    ledger: &Arc<Mutex<Ledger>>,
    read_size: usize,
) -> Vec<u8> {
    let transport =
        Transport::new(SliceReadAt::new(file), "object".into(), cfg, ledger.clone()).unwrap();
    let mut source = DecodeSource::new(
        &codecs(),
        &DecodeRequest::new(),
        transport,
        PolicySet::new(),
        DecodeOptions::new(StorageBudget::new(64 << 20, 64))
            .with_queue_slots(256)
            .with_drive(DriveOptions::new(0, read_size)),
    )
    .unwrap();
    drain(&mut source)
}

/// Per codec, so one codec's abort does not hide the other's result.
fn sequential_rounds(name: &str) {
    let file = fixture(name);
    let codecs = codecs();
    let reference = drain(
        &mut DecodeSource::from_slice(
            &codecs,
            &file,
            DecodeOptions::new(StorageBudget::new(64 << 20, 64)).with_queue_slots(256),
        )
        .unwrap(),
    );
    let len = file.len() as u64;
    let whole_ms = |rtt: f64| rtt + len as f64 / BYTES_PER_MS;
    for rtt in [0.0, 160.0] {
        for read_size in [4096usize, 65_536, 1 << 20] {
            let ledger = Arc::new(Mutex::new(Ledger::default()));
            let decoded = decode_through(&file, config(rtt, 0, false), &ledger, read_size);
            assert_eq!(decoded, reference, "{name} read_size {read_size}");
            let l = ledger.lock().unwrap();
            // No byte is requested twice, and nothing past the object.
            assert_eq!(l.transferred_bytes, len, "{name} read_size {read_size}");
            assert_eq!(l.requested_bytes, len, "{name} read_size {read_size}");
            eprintln!(
                "calculated\t{name}\t{len} B\trtt {rtt} ms\tread_size {read_size}\trounds {}\tmodel {:.3} ms\twhole-object GET {:.3} ms",
                l.rounds,
                l.now_ms,
                whole_ms(rtt)
            );
            assert!(
                l.rounds as u64 >= len.div_ceil(read_size as u64).min(len),
                "{name}"
            );
        }
    }
}

#[test]
fn sequential_decode_rounds_follow_read_size_and_never_refetch_png() {
    sequential_rounds("png");
}

#[test]
fn sequential_decode_rounds_follow_read_size_and_never_refetch_jpeg() {
    sequential_rounds("jpeg");
}

fn second_view(name: &str) {
    let file = fixture(name);
    let len = file.len() as u64;
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let first = decode_through(&file, config(160.0, len, true), &ledger, 65_536);
    let after_first = ledger.lock().unwrap().transferred_bytes;
    let second = decode_through(&file, config(160.0, len, true), &ledger, 65_536);
    let l = ledger.lock().unwrap();
    assert_eq!(first, second, "{name}");
    assert_eq!(after_first, len, "{name}");
    assert_eq!(
        l.transferred_bytes, len,
        "{name}: the second view fetched again"
    );
    assert_eq!(l.cache_hit_bytes, len, "{name}");
}

#[test]
fn a_second_view_of_the_same_object_reuses_the_cache_png() {
    second_view("png");
}

#[test]
fn a_second_view_of_the_same_object_reuses_the_cache_jpeg() {
    second_view("jpeg");
}

#[test]
fn detected_format_matches_fixture() {
    let codecs = codecs();
    for (name, file) in fixtures() {
        let expected = if name == "png" {
            ImageFormat::Png
        } else {
            ImageFormat::Jpeg
        };
        assert_eq!(codecs.detect(&file), Some(expected));
    }
}
