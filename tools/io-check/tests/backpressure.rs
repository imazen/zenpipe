//! Output schedules, staging and partial publication through `zenpipe::io`.
//!
//! Managed bytes are sampled from the shared `StorageBudget` at every writer
//! call. They cover the driver's row buffer and compressed staging only; the
//! encoder's native buffering is not part of that allowance.

use std::io::{self, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{OutputSchedule, PolicyRequirement, PolicySet, RequirementEvidence};
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, NoPulse};
use zencodec_io::row_encode::{self, Options, Refusal, Stage};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};
use zenpipe::{PipeResult, Source, Strip};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 65;

fn png() -> Vec<u8> {
    let mut samples = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            samples.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, ((x * 7) ^ (y * 5)) as u8]);
        }
    }
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec()
}

fn codecs() -> CodecSet {
    CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new())
}

struct Counted<S> {
    inner: S,
    rows: Arc<AtomicU64>,
    fail_first: bool,
}

impl<S: Source> Source for Counted<S> {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        if self.fail_first {
            return Err(whereat::At::from(zenpipe::PipeError::Op(
                "source gone".into(),
            )));
        }
        let strip = self.inner.next()?;
        if let Some(strip) = &strip {
            self.rows
                .fetch_add(u64::from(strip.rows()), Ordering::SeqCst);
        }
        Ok(strip)
    }
    fn width(&self) -> u32 {
        self.inner.width()
    }
    fn height(&self) -> u32 {
        self.inner.height()
    }
    fn format(&self) -> zenpipe::PixelFormat {
        self.inner.format()
    }
    fn color_context(&self) -> Option<std::sync::Arc<zenpixels::ColorContext>> {
        self.inner.color_context()
    }
}

fn rows(
    file: &[u8],
    rows: Arc<AtomicU64>,
    fail_first: bool,
) -> PipelineRows<Counted<DecodeSource<zencodec::source::SliceReadAt<'_>>>> {
    let decode = DecodeSource::from_slice(
        &codecs(),
        file,
        DecodeOptions::new(StorageBudget::new(64 << 20, 64))
            .with_strip_rows(8)
            .with_queue_slots(256),
    )
    .unwrap();
    PipelineRows::from_source(Counted {
        inner: decode,
        rows,
        fail_first,
    })
    .unwrap()
}

/// Records what was true at each write: rows read, managed bytes held.
struct Probe {
    out: Vec<u8>,
    rows: Arc<AtomicU64>,
    budget: StorageBudget,
    rows_at_first_write: Option<u64>,
    max_reserved: usize,
    fail_at: Option<usize>,
}

impl Write for Probe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_at.is_some_and(|limit| self.out.len() >= limit) {
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "peer gone"));
        }
        self.rows_at_first_write
            .get_or_insert(self.rows.load(Ordering::SeqCst));
        self.max_reserved = self.max_reserved.max(self.budget.reserved_bytes());
        let n = match self.fail_at {
            Some(limit) => bytes.len().min(limit - self.out.len()),
            None => bytes.len(),
        };
        self.out.extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn probe(budget: &StorageBudget, rows: &Arc<AtomicU64>, fail_at: Option<usize>) -> Probe {
    Probe {
        out: Vec::new(),
        rows: rows.clone(),
        budget: budget.clone(),
        rows_at_first_write: None,
        max_reserved: 0,
        fail_at,
    }
}

const BATCH: u32 = 4;

fn options(budget: &StorageBudget, max_output: u64) -> Options {
    Options::new(budget.clone(), max_output).with_batch_rows(NonZeroU32::new(BATCH).unwrap())
}

#[test]
fn schedules_trade_first_output_against_staged_bytes() {
    let file = png();
    let row_buffer = (W * 3 * BATCH) as usize;
    let mut results = Vec::new();
    for schedule in [OutputSchedule::Live, OutputSchedule::FinishFirst] {
        let counter = Arc::new(AtomicU64::new(0));
        let budget = StorageBudget::new(16 << 20, 1024);
        let mut sink = probe(&budget, &counter, None);
        let done = row_encode::encode_job(
            zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
            rows(&file, counter.clone(), false),
            &mut sink,
            PolicySet::new().require_output_schedule(schedule),
            options(&budget, 16 << 20),
            &NoPulse,
        )
        .unwrap();
        assert_eq!(done.schedule, schedule);
        eprintln!(
            "{schedule:?}: first write after {} of {H} rows; managed bytes held at writes <= {}; output {} B",
            sink.rows_at_first_write.unwrap(),
            sink.max_reserved,
            sink.out.len()
        );
        results.push(sink);
    }
    let (live, staged) = (&results[0], &results[1]);
    assert_eq!(live.out, staged.out);
    // Live: the header is published before any source row is read, and the
    // driver holds only its row buffer while a write waits.
    assert_eq!(live.rows_at_first_write, Some(0));
    assert_eq!(live.max_reserved, row_buffer);
    // FinishFirst: nothing is published until the encoder is done and the row
    // buffer is gone; the staged compressed output is held instead.
    assert_eq!(staged.rows_at_first_write, Some(u64::from(H)));
    assert!(
        staged.max_reserved >= staged.out.len(),
        "{}",
        staged.max_reserved
    );
}

#[test]
fn writer_failure_reports_the_confirmed_partial_publication() {
    let file = png();
    for schedule in [OutputSchedule::Live, OutputSchedule::FinishFirst] {
        let counter = Arc::new(AtomicU64::new(0));
        let budget = StorageBudget::new(16 << 20, 1024);
        let mut sink = probe(&budget, &counter, Some(100));
        let error = row_encode::encode_job(
            zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
            rows(&file, counter.clone(), false),
            &mut sink,
            PolicySet::new().require_output_schedule(schedule),
            options(&budget, 16 << 20),
            &NoPulse,
        )
        .unwrap_err();
        assert_eq!(error.error().stage, Stage::Write, "{schedule:?}");
        assert_eq!(error.error().emitted, 100, "{schedule:?}");
        assert_eq!(sink.out.len(), 100);
        let cause = zencodec::find_cause::<io::Error>(&error).unwrap();
        assert_eq!(cause.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            budget.reserved_bytes(),
            0,
            "managed storage outlived the failure"
        );
    }
}

#[test]
fn a_source_failure_after_the_live_header_leaves_a_partial_publication() {
    let file = png();
    let mut emitted = Vec::new();
    for schedule in [OutputSchedule::Live, OutputSchedule::FinishFirst] {
        let counter = Arc::new(AtomicU64::new(0));
        let budget = StorageBudget::new(16 << 20, 1024);
        let mut sink = probe(&budget, &counter, None);
        let error = row_encode::encode_job(
            zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
            rows(&file, counter.clone(), true),
            &mut sink,
            PolicySet::new().require_output_schedule(schedule),
            options(&budget, 16 << 20),
            &NoPulse,
        )
        .unwrap_err();
        assert_eq!(error.error().stage, Stage::Read, "{schedule:?}");
        assert_eq!(error.error().emitted, sink.out.len() as u64);
        emitted.push(sink.out.len());
    }
    assert!(emitted[0] > 0, "live published nothing before reading");
    assert_eq!(emitted[1], 0, "finish-first published before finishing");
}

#[test]
fn the_output_limit_is_a_publication_limit_not_a_precheck() {
    let file = png();
    let mut emitted = Vec::new();
    for schedule in [OutputSchedule::Live, OutputSchedule::FinishFirst] {
        let counter = Arc::new(AtomicU64::new(0));
        let budget = StorageBudget::new(16 << 20, 1024);
        let mut sink = probe(&budget, &counter, None);
        let error = row_encode::encode_job(
            zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
            rows(&file, counter.clone(), false),
            &mut sink,
            PolicySet::new().require_output_schedule(schedule),
            options(&budget, 200),
            &NoPulse,
        )
        .unwrap_err();
        assert_eq!(
            zencodec::find_cause::<Refusal>(&error),
            Some(&Refusal::OutputLimit),
            "{schedule:?}"
        );
        emitted.push(error.error().emitted);
    }
    assert!(emitted[0] > 0 && emitted[0] <= 200, "live: {}", emitted[0]);
    assert_eq!(emitted[1], 0);
}

#[test]
fn disk_staging_publishes_the_same_bytes_after_finishing() {
    let file = png();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("io_backpressure");
    std::fs::create_dir_all(&dir).unwrap();
    let scratch_path = dir.join("staging.bin");
    let scratch = std::fs::File::options()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&scratch_path)
        .unwrap();
    let counter = Arc::new(AtomicU64::new(0));
    let budget = StorageBudget::new(16 << 20, 1024);
    let mut sink = probe(&budget, &counter, None);
    let done = row_encode::encode_job(
        zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
        rows(&file, counter.clone(), false),
        &mut sink,
        PolicySet::new().require_output_schedule(OutputSchedule::FinishFirst),
        options(&budget, 16 << 20)
            .with_disk_staging(scratch, NonZeroUsize::new(512).unwrap())
            .unwrap(),
        &NoPulse,
    )
    .unwrap();
    assert_eq!(done.schedule, OutputSchedule::FinishFirst);
    assert_eq!(sink.rows_at_first_write, Some(u64::from(H)));
    // Only the bounded replay buffer is managed memory during writes.
    assert_eq!(sink.max_reserved, 512);
    assert_eq!(std::fs::read(&scratch_path).unwrap(), sink.out);
}

/// Expected to fail at zencodec `f5a7fd46` (policy composition gap): the
/// requirement passed only as the resolver is refused before any effect, but
/// the report names nothing, because the core admits and names requirements
/// from the driver's preset alone. Kept unchanged as the visible finding; the
/// preset form below is the working placement.
#[test]
fn a_required_wait_bound_is_unknown_and_refuses_before_any_effect() {
    let file = png();
    let counter = Arc::new(AtomicU64::new(0));
    let budget = StorageBudget::new(16 << 20, 1024);
    let mut sink = probe(&budget, &counter, None);
    let error = row_encode::encode_job(
        zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
        rows(&file, counter.clone(), false),
        &mut sink,
        PolicySet::new().with_output_wait_limit(1 << 30),
        options(&budget, 16 << 20),
        &NoPulse,
    )
    .unwrap_err();
    assert_eq!(error.error().stage, Stage::Policy);
    assert!(sink.out.is_empty());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    let unmet: Vec<_> = error
        .error()
        .report
        .decisions()
        .flat_map(|d| d.unmet_requirements.iter())
        .collect();
    assert!(
        unmet
            .iter()
            .any(|(r, _)| *r == PolicyRequirement::OutputWait),
        "{unmet:?}"
    );
}

/// The compatibility placement: the requirement also in the driver's deprecated
/// preset (and the same set as the operation policy). The two sets are ANDed;
/// the operation still refuses before any effect and names `OutputWait` with
/// `Unknown` evidence.
#[test]
fn a_required_wait_bound_in_the_preset_is_named_before_any_effect() {
    let file = png();
    let counter = Arc::new(AtomicU64::new(0));
    let budget = StorageBudget::new(16 << 20, 1024);
    let mut sink = probe(&budget, &counter, None);
    let bound = PolicySet::new().with_output_wait_limit(1 << 30);
    #[allow(
        deprecated,
        reason = "exercises the migration path: a preset still ANDs its requirements"
    )]
    let preset = options(&budget, 16 << 20).with_policy_set(bound);
    let error = row_encode::encode_job(
        zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
        rows(&file, counter.clone(), false),
        &mut sink,
        bound,
        preset,
        &NoPulse,
    )
    .unwrap_err();
    assert_eq!(error.error().stage, Stage::Policy);
    assert!(sink.out.is_empty());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    let unmet: Vec<_> = error
        .error()
        .report
        .decisions()
        .flat_map(|d| d.unmet_requirements.iter())
        .collect();
    assert!(
        unmet.contains(&(PolicyRequirement::OutputWait, RequirementEvidence::Unknown)),
        "{unmet:?}"
    );
}
