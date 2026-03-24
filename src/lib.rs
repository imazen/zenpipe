#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "std"), no_std)]

//! Streaming pixel pipeline — zero-materialization graph execution.
//!
//! Composes zen crate operations (decode, resize, color convert, composite)
//! into pull-based strip pipelines. Each operation pulls strips from its
//! upstream source, transforms them, and yields output strips. Only the
//! rows needed by the current kernel window are buffered.
//!
//! # Feature: `std` (default)
//!
//! Enables zenfilters (photo filters) and moxcms (ICC color management).
//! Without `std`, the core pipeline works in `no_std + alloc`: resize,
//! blend, codec bridge, animation, format conversion, limits.

extern crate alloc;

mod error;
pub mod format;
pub mod graph;
pub mod limits;
pub mod ops;
pub mod sources;
mod strip;

#[cfg(feature = "zenode")]
pub mod bridge;

#[cfg(feature = "zenode")]
pub mod orchestrate;

// Re-export bridge config types at crate root for convenience.
#[cfg(feature = "zenode")]
pub use bridge::{DagNode, DecodeConfig, EncodeConfig, PipelineResult};

pub mod animation;
pub mod codec;
pub mod sidecar;

pub use error::PipeError;
pub use format::PixelFormat;
pub use graph::{ResourceEstimate, SourceInfo};
pub use limits::Limits;
pub use strip::{Strip, StripBuf};

// Re-export key zenpixels-convert types.
pub use zenpixels_convert::{
    AlphaMode, ChannelLayout, ChannelType, ColorPrimaries, PixelDescriptor, RowConverter,
    SignalRange, TransferFunction,
};

// Re-export cancellation types.
pub use enough::{Stop, Unstoppable};

// Re-export CMS types (std only — moxcms requires std).
#[cfg(feature = "std")]
pub use zenpixels_convert::cms::{ColorManagement, RowTransform};
#[cfg(feature = "std")]
pub use zenpixels_convert::cms_moxcms::MoxCms;

/// A source of pixel strips (pull-based).
pub trait Source: Send {
    /// Pull the next strip. Returns `None` when the image is exhausted.
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError>;
    /// Output image width in pixels.
    fn width(&self) -> u32;
    /// Total output image height in pixels.
    fn height(&self) -> u32;
    /// Pixel format of output strips.
    fn format(&self) -> PixelFormat;
}

/// A sink that consumes pixel strips (push-based).
pub trait Sink: Send {
    /// Consume one strip of pixel data.
    fn consume(&mut self, strip: &Strip<'_>) -> Result<(), PipeError>;
    /// Signal end of image.
    fn finish(&mut self) -> Result<(), PipeError>;
}

/// Drive a pipeline: pull all strips from `source` into `sink`.
pub fn execute(source: &mut dyn Source, sink: &mut dyn Sink) -> Result<(), PipeError> {
    execute_with_stop(source, sink, &Unstoppable)
}

/// Drive a pipeline with cooperative cancellation.
pub fn execute_with_stop(
    source: &mut dyn Source,
    sink: &mut dyn Sink,
    stop: &dyn Stop,
) -> Result<(), PipeError> {
    while let Some(strip) = source.next()? {
        stop.check().map_err(PipeError::from)?;
        sink.consume(&strip)?;
    }
    sink.finish()
}
