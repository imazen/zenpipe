#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "std"), no_std)]

//! Streaming pixel pipeline — zero-materialization graph execution.
//!
//! Composes zen crate operations (decode, resize, color convert, composite)
//! into pull-based strip pipelines. Each operation pulls strips from its
//! upstream source, transforms them, and yields output strips. Only the
//! rows needed by the current kernel window are buffered.
//!
//! # Pixel format model
//!
//! Pixel formats are described by [`PixelFormat`] (an alias for
//! [`zenpixels_convert::PixelDescriptor`]), which carries color primaries
//! (BT.709, Display P3, BT.2020), transfer function (sRGB, Linear, PQ, HLG),
//! alpha mode, and channel layout. Format conversions are handled automatically
//! by the graph compiler via [`zenpixels_convert::RowConverter`].
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
pub mod format;
pub mod graph;
pub mod ops;
pub mod sources;
mod strip;

#[cfg(feature = "codec")]
pub mod codec;

pub use error::PipeError;
pub use format::{PixelFormat, PixelFormatExt};
pub use strip::{StripBuf, StripRef};

// Re-export key zenpixels-convert types for convenience.
pub use zenpixels_convert::{
    AlphaMode, ChannelLayout, ChannelType, ColorPrimaries, PixelDescriptor, RowConverter,
    SignalRange, TransferFunction,
};

// Re-export CMS types when the cms feature is enabled.
#[cfg(feature = "cms")]
pub use zenpixels_convert::cms::{ColorManagement, RowTransform};
#[cfg(feature = "cms")]
pub use zenpixels_convert::cms_moxcms::MoxCms;

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
