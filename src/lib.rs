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

whereat::define_at_crate_info!();
#[allow(unused_imports)]
use whereat::at;

mod error;
pub mod format;
pub mod graph;
pub mod limits;
pub mod ops;
pub mod sources;
mod strip;
#[cfg(feature = "std")]
pub mod trace;

#[cfg(feature = "zennode")]
pub mod bridge;

#[cfg(feature = "job")]
pub mod job;

#[cfg(feature = "json-schema")]
pub mod codegen_csharp;
#[cfg(feature = "json-schema")]
pub mod schema_export;

#[cfg(feature = "zennode")]
pub mod orchestrate;

#[cfg(feature = "zennode")]
mod node_registry;
#[cfg(feature = "zennode")]
pub use node_registry::full_registry;

#[cfg(feature = "zennode")]
pub mod srcset;

#[cfg(feature = "zennode")]
pub mod zennode_defs;

// Re-export bridge config types at crate root for convenience.
#[cfg(feature = "zennode")]
pub use bridge::{DagNode, DecodeConfig, EncodeConfig, MaterializedImage, PipelineResult};

// Re-export ordering types for callers that need to control node order.
#[cfg(feature = "zennode")]
pub use bridge::{OptimizationLevel, canonical_sort, optimize_node_order};

// Re-export orchestration types.
#[cfg(feature = "zennode")]
pub use orchestrate::{ProcessConfig, ProcessedImage, SourceImageInfo, StreamingOutput};

// Re-export zencodecs quality types for callers resolving encode quality.
#[cfg(feature = "zennode")]
pub use zencodecs::quality::{QualityIntent, QualityProfile};

pub mod animation;
pub mod codec;
#[cfg(feature = "json-schema")]
pub mod codec_info;
#[cfg(feature = "lossless-jpeg")]
pub mod lossless;
#[cfg(feature = "imageflow-compat")]
pub mod imageflow_compat;
pub mod sidecar;
pub mod watermark;

pub use error::{PipeError, PipeResult};
pub use format::PixelFormat;
pub use graph::{ResourceEstimate, SourceInfo};
#[cfg(feature = "std")]
pub use limits::Deadline;
pub use limits::{AllocationGuard, AllocationTracker, Limits};
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
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>>;
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
    fn consume(&mut self, strip: &Strip<'_>) -> PipeResult<()>;
    /// Signal end of image.
    fn finish(&mut self) -> PipeResult<()>;
}

/// Drive a pipeline: pull all strips from `source` into `sink`.
pub fn execute(source: &mut dyn Source, sink: &mut dyn Sink) -> PipeResult<()> {
    execute_with_stop(source, sink, &Unstoppable)
}

/// Drive a pipeline with cooperative cancellation.
pub fn execute_with_stop(
    source: &mut dyn Source,
    sink: &mut dyn Sink,
    stop: &dyn Stop,
) -> PipeResult<()> {
    while let Some(strip) = source.next()? {
        stop.check().map_err(|_| at!(PipeError::Cancelled))?;
        sink.consume(&strip)?;
    }
    sink.finish()
}
