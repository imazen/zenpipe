//! Fixed admitted row storage; no size growth after the route is selected.
use super::*;
use zencodec::decode::{DecodeRowSink, RowStrip, SinkError};
use zencodec::encode::{DynEncoder, EncodeDeclaration, EncodePlan};
use zencodec::storage::OwnedStorage;
use zenpixels::{PixelView, PixelViewMut};

pub(super) struct PreparedRows {
    width: u32,
    height: u32,
    max_rows: u32,
    source: PixelFormat,
    row_plan: RowPlan,
    input: OwnedStorage<u8>,
    output: OwnedStorage<u8>,
    source_stride: usize,
    output_stride: usize,
}
impl PreparedRows {
    pub fn new(
        info: &ImageInfo,
        source: PixelFormat,
        row_plan: RowPlan,
        max_rows: u32,
        budget: &StorageBudget,
    ) -> Result<Self, Cause> {
        let max_rows = max_rows.min(info.height);
        let source_stride = (info.width as usize)
            .checked_mul(source.layout.bytes_per_pixel())
            .ok_or(Refusal::SizeOverflow)?;
        let output_stride = (info.width as usize)
            .checked_mul(row_plan.destination_format().layout.bytes_per_pixel())
            .ok_or(Refusal::SizeOverflow)?;
        let input = budget.bytes(
            source_stride
                .checked_mul(max_rows as usize)
                .ok_or(Refusal::SizeOverflow)?,
        )?;
        let mut output = budget.bytes(
            output_stride
                .checked_mul(max_rows as usize)
                .ok_or(Refusal::SizeOverflow)?,
        )?;
        // Check layout/alignment before the application selects this route.
        PixelView::new(
            input.as_slice(),
            info.width,
            max_rows,
            source_stride,
            &source,
        )?;
        PixelViewMut::new(
            output.as_mut_slice(),
            info.width,
            max_rows,
            output_stride,
            row_plan.destination_format(),
        )?;
        Ok(Self {
            width: info.width,
            height: info.height,
            max_rows,
            source,
            row_plan,
            input,
            output,
            source_stride,
            output_stride,
        })
    }
    pub fn output_format(&self) -> &PixelFormat {
        self.row_plan.destination_format()
    }
    pub fn admitted_bytes(&self) -> usize {
        self.input.as_slice().len() + self.output.as_slice().len()
    }
}
struct Running {
    rows: PreparedRows,
    encoder: Box<dyn DynEncoder>,
    /// Admitted plan for the declared output format.
    plan: EncodePlan,
    pending: u32,
    pushed: u32,
    begun: bool,
    finished: bool,
}
impl Running {
    fn flush(&mut self) -> Result<(), Cause> {
        if self.pending == 0 {
            return Ok(());
        }
        let rows = &mut self.rows;
        let src_bytes = rows.input.as_slice();
        let dst_bytes = rows.output.as_mut_slice();
        for r in 0..self.pending as usize {
            let src_start = r * rows.source_stride;
            let dst_start = r * rows.output_stride;
            rows.row_plan.convert_row_at(
                self.pushed + r as u32,
                &src_bytes[src_start..src_start + rows.source_stride],
                &mut dst_bytes[dst_start..dst_start + rows.output_stride],
                rows.width,
            )?;
        }
        let converted = PixelView::new(
            rows.output.as_slice(),
            rows.width,
            self.pending,
            rows.output_stride,
            rows.row_plan.destination_format(),
        )?;
        self.encoder.push_rows(converted)?;
        self.pushed += self.pending;
        self.pending = 0;
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct Destination(Option<Running>);
impl Destination {
    pub fn install(
        &mut self,
        rows: PreparedRows,
        encoder: Box<dyn DynEncoder>,
        plan: EncodePlan,
    ) -> Result<(), Cause> {
        if self.0.is_some() {
            return Err(Refusal::RepeatedDestination.into());
        }
        self.0 = Some(Running {
            rows,
            encoder,
            plan,
            pending: 0,
            pushed: 0,
            begun: false,
            finished: false,
        });
        Ok(())
    }
    pub fn finish_encoder(mut self) -> Result<EncodeOutput, Cause> {
        let running = self.0.take().ok_or(Refusal::MissingRoute)?;
        if !running.finished {
            return Err(Refusal::MissingEncoderFinish.into());
        }
        let Running { rows, encoder, .. } = running;
        drop(rows);
        encoder.finish()
    }
}
impl DecodeRowSink for Destination {
    fn max_buffer_rows(&self) -> core::num::NonZeroU32 {
        self.0
            .as_ref()
            .and_then(|run| core::num::NonZeroU32::new(run.rows.max_rows))
            .unwrap_or(core::num::NonZeroU32::MIN)
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        let run = self.0.as_mut().ok_or(Refusal::DecoderBeforeSelection)?;
        if run.begun
            || (width, height) != (run.rows.width, run.rows.height)
            || pixel_format != run.rows.source
        {
            return Err(Refusal::GeometryOrOrientation.into());
        }
        // Start exactly the admitted plan, with the rows' color declared
        // before the header exists; a changed startup refuses before output.
        let declaration = EncodeDeclaration::new(width, height, run.rows.output_format().clone());
        let observed = run.encoder.begin_declared(&declaration)?;
        run.plan.validate_started(&observed)?;
        run.begun = true;
        Ok(())
    }
    fn provide_next_buffer(
        &mut self,
        strip: RowStrip,
        pixel_format: PixelFormat,
    ) -> Result<PixelViewMut<'_>, SinkError> {
        let run = self.0.as_mut().ok_or(Refusal::DecoderBeforeSelection)?;
        let (y, width, height) = (strip.y, strip.width, strip.height);
        if !run.begun
            || run.finished
            || height == 0
            || height > run.rows.max_rows
            || width != run.rows.width
            || pixel_format != run.rows.source
            || y != run.pushed + run.pending
            || y.checked_add(height)
                .is_none_or(|end| end > run.rows.height)
        {
            return Err(Refusal::StripGeometry.into());
        }
        run.flush()?;
        run.pending = height;
        Ok(PixelViewMut::new(
            run.rows.input.as_mut_slice(),
            width,
            height,
            run.rows.source_stride,
            &run.rows.source,
        )?)
    }
    fn finish(&mut self) -> Result<(), SinkError> {
        let run = self.0.as_mut().ok_or(Refusal::DecoderBeforeSelection)?;
        if !run.begun || run.finished {
            return Err(Refusal::RepeatedDestination.into());
        }
        run.flush()?;
        if run.pushed != run.rows.height {
            return Err(Refusal::MissingRows.into());
        }
        run.finished = true;
        Ok(())
    }
}
