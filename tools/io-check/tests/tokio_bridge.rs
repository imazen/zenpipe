//! Planned pipelines driven from Tokio through `zenpipe::io::tokio_bridge`.
//!
//! The pipeline runs on the blocking pool; bytes come from an `AsyncSource`
//! and leave through an `AsyncWrite`. Row counts, not wall time, establish
//! when output first appears.

use std::error::Error;
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::AsyncWrite;
use tokio::runtime::Handle;
use zencodec::decode::BoxedError;
use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{DecodeRequest, OutputSchedule, PolicySet};
use zencodec::source::ReadAt;
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, NoPulse};
use zencodec_io::row_encode::{self, Completed, Options};
use zencodec_io::tokio_io::{AsyncSource, BytesSource};
use zenpipe::io::tokio_bridge::{BlockingReadAt, DriveError, drive_to_async_writer};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};
use zenpipe::{PipeResult, Source, Strip};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 65;

type Failure = Box<dyn Error + Send + Sync>;

fn png_fixture() -> Vec<u8> {
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

fn decode_options() -> DecodeOptions {
    DecodeOptions::new(StorageBudget::new(64 << 20, 64))
        .with_strip_rows(8)
        .with_queue_slots(256)
}

fn encode_options() -> Options {
    // Small batches so a streaming PNG emits many chunks.
    Options::new(StorageBudget::new(16 << 20, 64), 16 << 20)
        .with_batch_rows(NonZeroU32::new(4).unwrap())
}

struct Counted<S> {
    inner: S,
    rows: Arc<AtomicU64>,
}

impl<S: Source> Source for Counted<S> {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
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

/// The whole operation: async bytes -> controlled PNG decode -> planned PNG
/// row encode -> the bridge's channel writer.
fn pipeline<S: AsyncSource + 'static>(
    source: S,
    handle: Handle,
    schedule: OutputSchedule,
    rows: Arc<AtomicU64>,
    writer: impl Write,
) -> Result<Completed, Failure> {
    let codecs = codecs();
    let reader = BlockingReadAt::new(source, handle);
    let decode = DecodeSource::new(
        &codecs,
        &DecodeRequest::new(),
        reader,
        PolicySet::new(),
        decode_options(),
    )?;
    let counted = Counted {
        inner: decode,
        rows,
    };
    let rows = PipelineRows::from_source(counted)?;
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    Ok(row_encode::encode_job(
        job,
        rows,
        writer,
        PolicySet::new().require_output_schedule(schedule),
        encode_options(),
        &NoPulse,
    )?)
}

fn sync_reference() -> Vec<u8> {
    let file = png_fixture();
    let codecs = codecs();
    let decode = DecodeSource::from_slice(&codecs, &file, decode_options()).unwrap();
    let rows = PipelineRows::from_source(decode).unwrap();
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    let mut out = Vec::new();
    row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new(),
        encode_options(),
        &NoPulse,
    )
    .unwrap();
    out
}

async fn run_bridge(schedule: OutputSchedule) -> (Vec<u8>, u64) {
    let source = BytesSource(png_fixture().into());
    let handle = Handle::current();
    let rows = Arc::new(AtomicU64::new(0));
    let counted = rows.clone();
    let mut out = Vec::new();
    let (completed, delivered) = drive_to_async_writer(
        2,
        move |writer| pipeline(source, handle, schedule, counted, writer),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(completed.schedule, schedule);
    assert_eq!(delivered.bytes, out.len() as u64);
    (out, delivered.chunks)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_bytes_through_the_blocking_pool_match_the_sync_route() {
    let reference = sync_reference();
    for schedule in [OutputSchedule::FinishFirst, OutputSchedule::Live] {
        let (out, chunks) = run_bridge(schedule).await;
        assert_eq!(out, reference, "{schedule:?}");
        eprintln!(
            "bridge {schedule:?}: {} bytes in {chunks} chunks",
            out.len()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_current_thread_runtime_also_drives_the_bridge() {
    let (out, _) = run_bridge(OutputSchedule::Live).await;
    assert_eq!(out, sync_reference());
}

/// Records how many source rows had been read when each chunk arrived.
struct Observed {
    rows: Arc<AtomicU64>,
    at_first_chunk: Option<u64>,
    chunks: u64,
    delay: Duration,
    bytes: Vec<u8>,
    fail_after: Option<u64>,
}

impl AsyncWrite for Observed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.fail_after.is_some_and(|n| self.chunks >= n) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "peer gone",
            )));
        }
        if self.at_first_chunk.is_none() {
            self.at_first_chunk = Some(self.rows.load(Ordering::SeqCst));
        }
        self.chunks += 1;
        self.bytes.extend_from_slice(buf);
        if !self.delay.is_zero() {
            // A slow peer: block this task briefly per chunk.
            std::thread::sleep(self.delay);
        }
        let _ = cx;
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn observe(
    schedule: OutputSchedule,
    delay: Duration,
    fail_after: Option<u64>,
) -> (Observed, Result<Completed, String>) {
    let source = BytesSource(png_fixture().into());
    let handle = Handle::current();
    let rows = Arc::new(AtomicU64::new(0));
    let mut writer = Observed {
        rows: rows.clone(),
        at_first_chunk: None,
        chunks: 0,
        delay,
        bytes: Vec::new(),
        fail_after,
    };
    let counted = rows.clone();
    let result = drive_to_async_writer(
        1,
        move |w| pipeline(source, handle, schedule, counted, w),
        &mut writer,
    )
    .await;
    let result = result.map(|(c, _)| c).map_err(|e| match e {
        DriveError::Writer(e) => format!("writer: {:?}", e.kind()),
        DriveError::Operation(e) => format!("operation: {e}"),
        other => format!("{other}"),
    });
    (writer, result)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_output_starts_before_the_last_row_and_finish_first_after_it() {
    let (live, result) = observe(OutputSchedule::Live, Duration::ZERO, None).await;
    result.unwrap();
    let (staged, result) = observe(OutputSchedule::FinishFirst, Duration::ZERO, None).await;
    result.unwrap();
    let live_first = live.at_first_chunk.unwrap();
    let staged_first = staged.at_first_chunk.unwrap();
    eprintln!(
        "first chunk after {live_first} rows (Live) vs {staged_first} rows (FinishFirst) of {H}"
    );
    assert!(
        live_first < u64::from(H),
        "live output waited for the whole image"
    );
    assert_eq!(
        staged_first,
        u64::from(H),
        "finish-first published before finishing"
    );
    assert_eq!(live.bytes, staged.bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_peer_with_one_queued_chunk_completes() {
    let (slow, result) = observe(OutputSchedule::Live, Duration::from_millis(2), None).await;
    result.unwrap();
    assert_eq!(slow.bytes, sync_reference());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_peer_stops_the_operation_and_reports_the_writer() {
    for schedule in [OutputSchedule::Live, OutputSchedule::FinishFirst] {
        let (writer, result) = observe(schedule, Duration::ZERO, Some(2)).await;
        assert_eq!(
            result.unwrap_err(),
            "writer: ConnectionReset",
            "{schedule:?}"
        );
        assert_eq!(writer.chunks, 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_completion_before_draining_deadlocks_a_bounded_channel() {
    // The hazard drive_to_async_writer avoids: await the blocking work first,
    // drain later. More output than channel capacity can never complete.
    let source = BytesSource(png_fixture().into());
    let handle = Handle::current();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
    struct Chan(tokio::sync::mpsc::Sender<Vec<u8>>);
    impl Write for Chan {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0
                .blocking_send(b.to_vec())
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let rows = Arc::new(AtomicU64::new(0));
    let mut task = tokio::task::spawn_blocking(move || {
        pipeline(source, handle, OutputSchedule::Live, rows, Chan(tx))
    });
    let waited = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    assert!(waited.is_err(), "completed without a concurrent drain");
    // Releasing the receiver unblocks the writer with BrokenPipe.
    let first = rx.recv().await;
    assert!(first.is_some());
    drop(rx);
    let error = task.await.unwrap().unwrap_err();
    let cause = zencodec::find_cause::<io::Error>(&*error).expect("writer cause kept");
    assert_eq!(cause.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_future_ends_the_blocking_work() {
    let source = BytesSource(png_fixture().into());
    let handle = Handle::current();
    let ended = Arc::new(AtomicBool::new(false));
    let flag = ended.clone();
    let rows = Arc::new(AtomicU64::new(0));
    let mut writer = Observed {
        rows: rows.clone(),
        at_first_chunk: None,
        chunks: 0,
        delay: Duration::from_millis(20),
        bytes: Vec::new(),
        fail_after: None,
    };
    let fut = drive_to_async_writer(
        1,
        move |w| {
            let result = pipeline(source, handle, OutputSchedule::Live, rows, w);
            flag.store(true, Ordering::SeqCst);
            result
        },
        &mut writer,
    );
    let _ = tokio::time::timeout(Duration::from_millis(30), fut).await;
    // The future is gone; its channel receiver is closed.
    for _ in 0..200 {
        if ended.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("blocking work kept running after its future was dropped");
}

/// Serves each batch concurrently and completes the requests in reverse order.
struct ReverseCompletion(Arc<[u8]>);

impl AsyncSource for ReverseCompletion {
    async fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, BoxedError> {
        let start = (offset as usize).min(self.0.len());
        let n = (self.0.len() - start).min(out.len());
        out[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }
    async fn read_ranges(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), BoxedError> {
        // Every request is in flight at once; each waits for its release, and
        // releases go last-to-first, so completion order is deterministic.
        let mut set = tokio::task::JoinSet::new();
        let mut releases = Vec::new();
        for (i, (offset, out)) in reqs.iter().enumerate() {
            let bytes = self.0.clone();
            let (start, len) = (*offset as usize, out.len());
            let (release, wait) = tokio::sync::oneshot::channel::<()>();
            releases.push(release);
            set.spawn(async move {
                let _ = wait.await;
                (i, bytes[start..start + len].to_vec())
            });
        }
        while let Some(release) = releases.pop() {
            let expected = releases.len();
            let _ = release.send(());
            let (i, body) = set.join_next().await.expect("one task per request")?;
            assert_eq!(i, expected, "completion order");
            reqs[i].1.copy_from_slice(&body);
        }
        Ok(())
    }
    fn len_hint(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }
    async fn len(&mut self) -> Result<u64, BoxedError> {
        Ok(self.0.len() as u64)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_order_range_completion_lands_every_byte_in_place() {
    let data: Arc<[u8]> = (0..=255u8)
        .cycle()
        .take(64 * 1024)
        .collect::<Vec<_>>()
        .into();
    let handle = Handle::current();
    let source = ReverseCompletion(data.clone());
    tokio::task::spawn_blocking(move || {
        let mut reader = BlockingReadAt::new(source, handle);
        let (mut a, mut b, mut c) = (vec![0; 100], vec![0; 4096], vec![0; 7]);
        reader
            .read_many_at(&mut [(10, &mut a[..]), (20_000, &mut b[..]), (65_000, &mut c[..])])
            .unwrap();
        assert_eq!(&a[..], &data[10..110]);
        assert_eq!(&b[..], &data[20_000..24_096]);
        assert_eq!(&c[..], &data[65_000..65_007]);
    })
    .await
    .unwrap();
}
