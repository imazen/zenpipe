//! A pipeline served to the planned row encoder.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use whereat::at;
use zencodec::operation::OutputSource;
use zencodec_io::row_encode::{Description, Source as RowSource};
use zenpixels::{ColorContext, PixelSliceMut};

use super::Refusal;
use crate::Source;
use crate::error::{PipeError, PipeResult};

/// Counters for one [`PipelineRows`] run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RowStats {
    /// Strips pulled from the pipeline.
    pub strips: u64,
    /// Rows copied into the encode driver's buffers.
    pub rows: u64,
    /// Rows that passed through the carry buffer because a strip was taller
    /// than the rows the driver requested.
    pub carried_rows: u64,
    /// Peak carry-buffer capacity in bytes (not admitted storage).
    pub carry_capacity: usize,
    /// Strips without a color context (an operation did not propagate it).
    pub strips_without_color: u64,
}

fn refused(refusal: Refusal) -> whereat::At<PipeError> {
    at!(PipeError::Codec(Box::new(refusal)))
}

/// Serve a pipeline's output rows to [`zencodec_io::row_encode`].
///
/// The description is fixed at construction from the pipeline's geometry and
/// format plus the caller's color context. Each strip is checked against it:
/// a different layout refuses, and a strip that carries a different color
/// context refuses rather than relabeling samples. Strips without a context
/// are accepted and counted, because several pipeline operations do not yet
/// propagate one.
///
/// Use with [`row_encode::encode_job`](zencodec_io::row_encode::encode_job) or
/// [`encode_job_to_destination`](zencodec_io::row_encode::encode_job_to_destination):
/// the driver plans the job, admits schedule and staging, then pulls rows.
pub struct PipelineRows<S> {
    source: S,
    description: Description,
    row_bytes: usize,
    carry: Vec<u8>,
    carry_rows: u32,
    carry_start: u32,
    next_y: u32,
    stats: RowStats,
}

impl<S: Source> PipelineRows<S> {
    /// Fix the encode description from `source` and an explicitly declared
    /// interpretation of its samples. Pass the context the pipeline's output
    /// actually has; prefer [`from_source`](Self::from_source), which takes it
    /// from operators that vouch for it. `None` declares no color.
    pub fn new(source: S, color: Option<Arc<ColorContext>>) -> PipeResult<Self> {
        let (width, height, format) = (source.width(), source.height(), source.format());
        let row_bytes = crate::limits::checked_buffer_size(width, 1, format.bytes_per_pixel())?;
        let mut description = Description::new(width, height, format);
        if let Some(color) = color {
            description = description.with_color_context(color);
        }
        Ok(Self {
            source,
            description,
            row_bytes,
            carry: Vec::new(),
            carry_rows: 0,
            carry_start: 0,
            next_y: 0,
            stats: RowStats::default(),
        })
    }

    /// Declare the color the pipeline itself states through
    /// [`Source::color_context`]. Refuses with [`Refusal::UnknownColor`] when
    /// it cannot: an operator that may change the interpretation does not
    /// vouch for it, so the caller must declare it with [`new`](Self::new).
    pub fn from_source(source: S) -> PipeResult<Self> {
        let color = source
            .color_context()
            .ok_or_else(|| refused(Refusal::UnknownColor))?;
        Self::new(source, Some(color))
    }

    /// Counters so far.
    pub fn stats(&self) -> RowStats {
        self.stats
    }

    fn fill(
        &mut self,
        first: u32,
        rows: &mut PixelSliceMut<'_>,
        pulse: &dyn zencodec::Pulse,
    ) -> PipeResult<()> {
        if first != self.next_y
            || rows.width() != self.description.width
            || rows.descriptor() != self.description.descriptor
        {
            return Err(refused(Refusal::RowOrder));
        }
        let wanted = rows.rows();
        let row_bytes = self.row_bytes;
        let mut filled = 0;
        while filled < wanted {
            pulse
                .check()
                .map_err(|reason| at!(PipeError::Codec(Box::new(stopped(reason)))))?;
            if self.carry_rows > 0 {
                let take = self.carry_rows.min(wanted - filled);
                for r in 0..take {
                    let at = (self.carry_start + r) as usize * row_bytes;
                    rows.row_mut(filled + r)
                        .copy_from_slice(&self.carry[at..at + row_bytes]);
                }
                self.carry_start += take;
                self.carry_rows -= take;
                filled += take;
                continue;
            }
            let Some(strip) = self.source.next()? else {
                return Err(refused(Refusal::MissingRows));
            };
            let available = strip.rows();
            if strip.width() != self.description.width
                || strip.descriptor() != self.description.descriptor
                || available == 0
            {
                return Err(refused(Refusal::SourceChanged));
            }
            match (strip.color_context(), &self.description.color_context) {
                (None, _) => self.stats.strips_without_color += 1,
                (Some(actual), Some(declared)) if actual == declared => {}
                (Some(_), _) => return Err(refused(Refusal::ColorChanged)),
            }
            // Copy what the driver asked for straight from the borrowed strip;
            // only the remainder has to outlive this borrow.
            let take = available.min(wanted - filled);
            for r in 0..take {
                rows.row_mut(filled + r).copy_from_slice(strip.row(r));
            }
            let rest = available - take;
            if rest > 0 {
                self.carry.clear();
                self.carry
                    .try_reserve(rest as usize * row_bytes)
                    .map_err(|e| at!(PipeError::Codec(Box::new(e))))?;
                for r in take..available {
                    self.carry.extend_from_slice(strip.row(r));
                }
                self.stats.carry_capacity = self.stats.carry_capacity.max(self.carry.capacity());
                self.stats.carried_rows += u64::from(rest);
            }
            self.stats.strips += 1;
            self.carry_rows = rest;
            self.carry_start = 0;
            filled += take;
        }
        self.next_y += wanted;
        self.stats.rows += u64::from(wanted);
        Ok(())
    }
}

fn stopped(reason: enough::StopReason) -> zencodec::CodecError {
    zencodec::CodecError::new(None, reason.into())
}

impl<S: Source> RowSource for PipelineRows<S> {
    type Error = whereat::At<PipeError>;

    fn description(&self) -> Description {
        self.description.clone()
    }

    fn mechanism(&self) -> OutputSource {
        OutputSource::PullRows
    }

    fn read_rows(
        &mut self,
        first: u32,
        mut rows: PixelSliceMut<'_>,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<(), Self::Error> {
        // Resolve absence only for the private fill loop's checkpoints.
        self.fill(first, &mut rows, pulse.unwrap_or(&zencodec::NoPulse))
    }
}
