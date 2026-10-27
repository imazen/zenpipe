//! Run planned, policy-admitted pipelines from a Tokio task.
//!
//! zencodec-io has no async row encoder: its planned driver
//! ([`row_encode`](zencodec_io::row_encode)) is synchronous, and its async
//! transcode ([`transcode_async`](zencodec_io::tokio_io::transcode_async))
//! neither plans the retained job nor resolves an output schedule. These
//! adapters run the synchronous pipeline on Tokio's blocking pool instead:
//!
//! - [`BlockingReadAt`] serves positional reads from an
//!   [`AsyncSource`] by blocking the pipeline's thread on the runtime handle.
//!   Batched requests stay one `read_ranges` call, so a concurrent source
//!   still overlaps them.
//! - [`drive_to_async_writer`] runs the pipeline with a [`ChannelWriter`] and
//!   drains its chunks into an `AsyncWrite` *concurrently*. A bounded channel
//!   that is only drained after the pipeline returns would deadlock as soon
//!   as the encoder's output exceeds its capacity, whatever the schedule.
//!
//! One blocking-pool thread is held for the whole operation, including while
//! a remote read is in flight; native work and reads are not interleaved
//! with other tasks on that thread. Never call these from a runtime worker
//! thread: `Handle::block_on` panics inside an async context.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use zencodec::decode::ErasedError;
use zencodec::source::ReadAt;
use zencodec_io::tokio_io::AsyncSource;

/// A positional source whose reads block on an [`AsyncSource`].
///
/// Use it only on a thread that may block (`tokio::task::spawn_blocking`).
/// Errors keep the async source's original cause.
pub struct BlockingReadAt<S> {
    source: S,
    handle: Handle,
}

impl<S> BlockingReadAt<S> {
    /// Drive `source` through `handle`, normally `Handle::current()` captured
    /// on the task that spawns the blocking work.
    pub fn new(source: S, handle: Handle) -> Self {
        Self { source, handle }
    }

    /// The async source.
    pub fn into_inner(self) -> S {
        self.source
    }
}

impl<S: AsyncSource> ReadAt for BlockingReadAt<S> {
    type Error = ErasedError;

    fn len(&mut self) -> Result<u64, ErasedError> {
        let source = &mut self.source;
        self.handle.block_on(source.len()).map_err(ErasedError::new)
    }

    fn len_hint(&self) -> Option<u64> {
        self.source.len_hint()
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, ErasedError> {
        let source = &mut self.source;
        self.handle
            .block_on(source.read_at(offset, out))
            .map_err(ErasedError::new)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), ErasedError> {
        self.read_many_at(&mut [(offset, out)])
    }

    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), ErasedError> {
        let source = &mut self.source;
        self.handle
            .block_on(source.read_ranges(reqs))
            .map_err(ErasedError::new)
    }
}

/// A synchronous writer that hands each write to an async task over a
/// bounded channel. A full channel blocks the writing thread; a closed one
/// (the receiving task stopped) fails with `BrokenPipe`.
pub struct ChannelWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

impl io::Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.tx
            .blocking_send(bytes.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Failure of [`drive_to_async_writer`].
#[derive(Debug)]
#[non_exhaustive]
pub enum DriveError<E> {
    /// The synchronous operation failed; its error is unchanged.
    Operation(E),
    /// The async writer failed. The operation saw a closed channel after this.
    Writer(io::Error),
    /// The blocking task panicked or was cancelled by the runtime.
    Join(tokio::task::JoinError),
}

impl<E: fmt::Display> fmt::Display for DriveError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operation(e) => write!(f, "pipeline operation failed: {e}"),
            Self::Writer(e) => write!(f, "async writer failed: {e}"),
            Self::Join(e) => write!(f, "blocking task failed: {e}"),
        }
    }
}

impl<E: core::error::Error + 'static> core::error::Error for DriveError<E> {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Operation(e) => Some(e),
            Self::Writer(e) => Some(e),
            Self::Join(e) => Some(e),
        }
    }
}

/// What reached the async writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Delivered {
    /// Bytes written to the async writer.
    pub bytes: u64,
    /// Chunks received from the operation.
    pub chunks: u64,
    /// Largest chunk.
    pub max_chunk: usize,
}

/// Run `operation` on the blocking pool with a [`ChannelWriter`] holding at
/// most `capacity` chunks, and drain those chunks into `writer` while it runs.
///
/// Returns the operation's result and what was delivered. When `writer`
/// fails, the channel closes, so the operation's next write fails with
/// `BrokenPipe`; the writer's error is returned after the operation ends.
/// Dropping this future closes the channel the same way; the blocking task
/// then finishes at its next write or pulse check, not immediately.
pub async fn drive_to_async_writer<T, E, F, W>(
    capacity: usize,
    operation: F,
    writer: &mut W,
) -> Result<(T, Delivered), DriveError<E>>
where
    F: FnOnce(ChannelWriter) -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
    W: AsyncWrite + Unpin + ?Sized,
{
    let (tx, mut rx) = mpsc::channel(capacity.max(1));
    let task = tokio::task::spawn_blocking(move || operation(ChannelWriter { tx }));
    let mut delivered = Delivered::default();
    let mut writer_error = None;
    while let Some(chunk) = rx.recv().await {
        if let Err(e) = writer.write_all(&chunk).await {
            writer_error = Some(e);
            break;
        }
        delivered.bytes += chunk.len() as u64;
        delivered.chunks += 1;
        delivered.max_chunk = delivered.max_chunk.max(chunk.len());
    }
    if writer_error.is_none()
        && let Err(e) = writer.flush().await
    {
        writer_error = Some(e);
    }
    drop(rx);
    let result = task.await.map_err(DriveError::Join)?;
    if let Some(error) = writer_error {
        return Err(DriveError::Writer(error));
    }
    let value = result.map_err(DriveError::Operation)?;
    Ok((value, delivered))
}

#[allow(unused)]
fn _assert_send<T: Send>() {}
#[allow(unused)]
fn _bridge_types_are_send() {
    _assert_send::<ChannelWriter>();
    _assert_send::<Box<DriveError<ErasedError>>>();
}
