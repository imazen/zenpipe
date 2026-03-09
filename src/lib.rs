#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "std"), no_std)]

//! Streaming pixel pipeline — zero-materialization graph execution.
//!
//! Composes zen crate operations (decode, resize, color convert, composite)
//! into pull-based strip pipelines. Each operation pulls strips from its
//! upstream source, transforms them, and yields output strips. Only the
//! rows needed by the current kernel window are buffered.
//!
//! # Execution model
//!
//! The pipeline is **pull-based**: the sink (encoder) drives execution by
//! requesting strips. Each [`Source`] pulls from its upstream source(s)
//! on demand. This naturally creates backpressure — upstream sources only
//! decode/compute rows when downstream needs them.
//!
//! # Strip batching
//!
//! Operations process horizontal strips (batches of scanlines) rather than
//! individual pixels or full images. Strip height is negotiated between
//! adjacent operations — codec MCU heights, resize kernel radii, and SIMD
//! widths all influence the optimal batch size.
//!
//! # Operation fusion
//!
//! Adjacent per-pixel operations (color conversion, premultiply, swizzle,
//! color filters) are fused into a single pass over each strip via
//! [`TransformSource`](sources::TransformSource). No intermediate buffers
//! between fused operations.

extern crate alloc;

mod error;
mod format;
pub mod ops;
pub mod sources;
mod strip;

pub use error::PipeError;
pub use format::PixelFormat;
pub use strip::{StripBuf, StripRef};

/// A source of pixel strips (pull-based).
///
/// Each call to [`next`](Source::next) yields the next horizontal strip
/// of the output image. The returned [`StripRef`] borrows from the source's
/// internal buffer and is invalidated by the next call.
///
/// Sources form a chain: each source pulls from its upstream source(s),
/// transforms the data, and yields output strips.
pub trait Source: Send {
    /// Pull the next strip. Returns `None` when the image is exhausted.
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError>;

    /// Output image width in pixels.
    fn width(&self) -> u32;

    /// Total output image height in pixels.
    fn height(&self) -> u32;

    /// Pixel format of output strips.
    fn format(&self) -> PixelFormat;
}

/// A sink that consumes pixel strips (push-based).
///
/// Used to adapt push-based consumers (encoders) to the pull-based
/// pipeline model. Call [`execute`] to drive the pipeline.
pub trait Sink: Send {
    /// Consume one strip of pixel data.
    fn consume(&mut self, strip: &StripRef<'_>) -> Result<(), PipeError>;

    /// Signal end of image.
    fn finish(&mut self) -> Result<(), PipeError>;
}

/// Drive a pipeline: pull all strips from `source` into `sink`.
pub fn execute(source: &mut dyn Source, sink: &mut dyn Sink) -> Result<(), PipeError> {
    while let Some(strip) = source.next()? {
        sink.consume(&strip)?;
    }
    sink.finish()
}
