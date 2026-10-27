//! `zenpipe::io` decoding over real HTTP/1.1 range requests on loopback with
//! a strict client: request accounting, object replacement, truncation,
//! range-ignoring servers and cancellation. Protocol qualification only.

#[path = "support/http.rs"]
mod http;

use std::sync::atomic::{AtomicU64, Ordering};

use http::{FailureKind, Fault, HttpFailure, Server, StrictHttp};
use zencodec::decode::{ErasedError, StepError};
use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::how_far::StopOnly;
use zencodec::operation::{DecodeRequest, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, SharedPulse};
use zencodec_io::DriveOptions;
use zenpipe::Source;
use zenpipe::io::{DecodeOptions, DecodeSource};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 200;
const H: u32 = 150;

fn png() -> (Vec<u8>, Vec<u8>) {
    let mut state = 0x2545_f491u32;
    let samples: Vec<u8> = (0..W * H * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect();
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    let file = zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec();
    (file, samples)
}

fn codecs() -> CodecSet {
    CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new())
}

fn options(read_size: usize) -> DecodeOptions {
    DecodeOptions::new(StorageBudget::new(64 << 20, 64))
        .with_queue_slots(256)
        .with_drive(DriveOptions::new(0, read_size))
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

fn open(file: &[u8], fault: Fault, read_size: usize) -> (Server, zenpipe::PipeResult<Vec<u8>>) {
    let server = Server::start(file.to_vec().into(), fault);
    let result = StrictHttp::open(server.port)
        .map_err(|e| whereat::At::from(zenpipe::PipeError::Codec(Box::new(e))))
        .and_then(|source| {
            let mut decode = DecodeSource::new(
                &codecs(),
                &DecodeRequest::new(),
                source,
                PolicySet::new(),
                options(read_size),
            )?;
            Ok(drain(&mut decode))
        });
    (server, result)
}

fn failure(error: &whereat::At<zenpipe::PipeError>) -> FailureKind {
    zencodec::find_cause::<HttpFailure>(error)
        .unwrap_or_else(|| panic!("no HTTP cause in {error:?}"))
        .kind
}

#[test]
fn ranged_decode_matches_authored_samples_and_validates_every_response() {
    let (file, samples) = png();
    for read_size in [16 * 1024, 256 * 1024] {
        let (server, decoded) = open(&file, Fault::None, read_size);
        assert_eq!(decoded.unwrap(), samples, "read_size {read_size}");
        let log = server.requests();
        assert!(log.iter().all(|r| r.status == 206), "{log:?}");
        assert!(log[0].if_match.is_none());
        assert!(
            log[1..].iter().all(|r| r.if_match.is_some()),
            "conditional read omitted"
        );
        let body: usize = log.iter().map(|r| r.body).sum();
        // The one-byte open probe is read again by the decoder's first round.
        assert_eq!(body, file.len() + 1, "read_size {read_size}");
        eprintln!(
            "http\t{} B object\tread_size {read_size}\trequests {}\tbody bytes {body}",
            file.len(),
            log.len()
        );
    }
}

#[test]
fn a_replaced_object_refuses_instead_of_mixing_versions() {
    let (file, _) = png();
    let (server, decoded) = open(&file, Fault::ChangeFrom(3), 16 * 1024);
    let error = decoded.unwrap_err();
    assert_eq!(failure(&error), FailureKind::Status(412));
    let log = server.requests();
    assert_eq!(log.len(), 3, "a request after the failure: {log:?}");
    assert_eq!(log[2].status, 412);
}

#[test]
fn a_truncated_body_is_a_typed_failure() {
    let (file, _) = png();
    let (server, decoded) = open(&file, Fault::ShortFrom(2), 16 * 1024);
    assert_eq!(failure(&decoded.unwrap_err()), FailureKind::Body);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_full_response_to_a_range_request_refuses() {
    let (file, _) = png();
    let (server, decoded) = open(&file, Fault::FullFrom(2), 16 * 1024);
    assert_eq!(failure(&decoded.unwrap_err()), FailureKind::Status(200));
    assert_eq!(server.requests().len(), 2);
}

struct TripAfter {
    checks: AtomicU64,
    after: u64,
}

impl enough::Stop for TripAfter {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.checks.fetch_add(1, Ordering::SeqCst) >= self.after {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn cancellation_keeps_its_reason_and_stops_requests() {
    let (file, _) = png();
    let server = Server::start(file.clone().into(), Fault::None);
    let source = StrictHttp::open(server.port).unwrap();
    let pulse = SharedPulse::new(StopOnly::new(TripAfter {
        checks: AtomicU64::new(0),
        after: 3,
    }));
    let error = DecodeSource::new(
        &codecs(),
        &DecodeRequest::new(),
        source,
        PolicySet::new(),
        options(4096).with_pulse(pulse),
    )
    .err()
    .expect("cancelled");
    let step = zencodec::find_cause::<StepError<ErasedError>>(&error)
        .unwrap_or_else(|| panic!("no step error in {error:?}"));
    assert!(
        matches!(step, StepError::Cancelled(enough::StopReason::Cancelled)),
        "{step:?}"
    );
    let at_failure = server.requests().len();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(
        server.requests().len(),
        at_failure,
        "requests after cancellation"
    );
    // The object is many 4 KiB rounds long; cancellation came well before the end.
    assert!(at_failure < file.len() / 4096, "{at_failure} requests");
}
