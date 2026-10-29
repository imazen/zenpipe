//! Pipelines over zencodec-io drivers: decode from slices, files and range
//! sources, then encode through the planned, policy-admitted row driver.
//!
//! [`DecodeSource`] runs a controlled zencodec decoder over any
//! [`ReadAt`](zencodec::source::ReadAt) (a slice through
//! [`SliceReadAt`](zencodec::source::SliceReadAt), a
//! [`FileReadAt`](zencodec_io::FileReadAt), an HTTP or object-store adapter)
//! and serves its rows as a pipeline [`Source`](crate::Source). The decoder
//! stays suspended between pulls; each pull steps it until one strip is ready.
//!
//! [`PipelineRows`] serves a pipeline's output to
//! [`zencodec_io::row_encode`]. Its job-based entry points plan the retained
//! encoder job, admit the output schedule and staging under the supplied
//! policy, and only then read the first row or publish the first byte.
//! A registry's `Box<dyn DynEncodeJob>` uses the same driver through
//! `zencodec_io::row_encode::encode_dyn_job`.
//!
//! Copies per decoded row: the native batch is copied into the controlled
//! decoder's delivery buffer, then into a held strip slot here; pipeline
//! operations read the slot in place; the encode driver's row buffer receives
//! one more copy from the pipeline's last strip. Strips are borrowed from
//! their upstream, so rows left over when strip and driver batch heights
//! differ are kept in a carry buffer of less than one strip.
//!
//! Neither adapter bounds native codec workspace, the controlled decoder's
//! retained compressed input, or encoder-internal buffering. Admitted storage
//! covers the strip slots, the encode driver's row buffer and its compressed
//! staging; [`DecodeSource::stats`] and [`PipelineRows::stats`] report what
//! the adapters actually held.

mod bytes;
mod decode;
mod encode;
mod orientation;
#[cfg(feature = "io-tokio")]
pub mod tokio_bridge;

pub use bytes::SharedBytes;
pub use decode::{DecodeOptions, DecodeSource, DecodeStats};
pub use encode::{PipelineRows, RowStats};
pub use orientation::{OrientationCost, render_orientation};

use core::fmt;

/// A contract the adapters could not establish. Native, source, policy and
/// cancellation causes keep their own types in the error chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// The decoder delivered more strips in one step than the queue holds.
    /// Raise [`DecodeOptions::with_queue_slots`] or the strip height; the
    /// queue never grows on its own.
    StripQueueFull {
        /// Admitted strip slots.
        slots: u32,
    },
    /// A strip was taller than the admitted strip height.
    StripTooTall {
        /// Rows the decoder asked to write.
        rows: u32,
        /// Admitted strip height.
        max: u32,
    },
    /// Rows arrived out of order, outside the image, or with another layout.
    StripGeometry,
    /// The decoder delivered rows without declaring their layout first.
    MissingDescriptor,
    /// The decoder finished before delivering every row.
    MissingRows,
    /// The pipeline's geometry or format differs from the encode description.
    SourceChanged,
    /// A strip carried a color context other than the declared one.
    ColorChanged,
    /// The pipeline cannot state the interpretation of its output (an
    /// operator that may change it does not vouch for it). Declare it with
    /// [`PipelineRows::new`].
    UnknownColor,
    /// The encode driver asked for rows out of order.
    RowOrder,
    /// A zero strip height or queue size cannot hold rows.
    InvalidOptions,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pipeline I/O refused: {self:?}")
    }
}

impl core::error::Error for Refusal {}
