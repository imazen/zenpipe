//! Fixed admitted row storage; no size growth after the route is selected.
use super::*;
use zencodec::decode::{DecodeRowSink, SinkError};
use zencodec::encode::{DynEncoder, EncodeDeclaration, EncodePlan};
use zencodec::storage::OwnedStorage;
use zenpixels::{ColorContext, PixelSlice, PixelSliceMut};

/// The interpretation of the rows the conversion will produce, declared to
/// the encoder before its header exists. zenpixels-convert keeps its own copy
/// private (`OutputPlan::output_context` is `pub(super)`), so this rebuilds it
/// from the public carriers; the first converted batch is checked against it.
/// HDR routes refuse earlier, so diffuse white only carries over.
pub(super) fn output_context(conversion: &OutputPlan, current: &ColorContext) -> ColorContext {
    let mut context = ColorContext::default();
    context.icc = conversion.metadata().icc.clone();
    context.cicp = conversion.metadata().cicp;
    context.diffuse_white = current.diffuse_white;
    context
}

pub(super) struct PreparedRows {
    width: u32,
    height: u32,
    max_rows: u32,
    source: PixelDescriptor,
    current: Arc<ColorContext>,
    conversion: OutputPlan,
    input: OwnedStorage<u8>,
    output: OwnedStorage<u8>,
    source_stride: usize,
    output_stride: usize,
}
impl PreparedRows {
    pub fn new(
        info: &ImageInfo,
        source: PixelDescriptor,
        current: Arc<ColorContext>,
        conversion: OutputPlan,
        max_rows: u32,
        budget: &StorageBudget,
    ) -> Result<Self, Cause> {
        let max_rows = max_rows.min(info.height);
        let source_stride = (info.width as usize)
            .checked_mul(source.bytes_per_pixel())
            .ok_or(Refusal::SizeOverflow)?;
        let output_stride = (info.width as usize)
            .checked_mul(conversion.output_descriptor().bytes_per_pixel())
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
        PixelSlice::new(
            input.as_slice(),
            info.width,
            max_rows,
            source_stride,
            source,
        )?;
        PixelSliceMut::new(
            output.as_mut_slice(),
            info.width,
            max_rows,
            output_stride,
            conversion.output_descriptor(),
        )?;
        Ok(Self {
            width: info.width,
            height: info.height,
            max_rows,
            source,
            current,
            conversion,
            input,
            output,
            source_stride,
            output_stride,
        })
    }
    pub fn output_descriptor(&self) -> PixelDescriptor {
        self.conversion.output_descriptor()
    }
    pub fn admitted_bytes(&self) -> usize {
        self.input.as_slice().len() + self.output.as_slice().len()
    }
}
struct Running {
    rows: PreparedRows,
    encoder: Box<dyn DynEncoder>,
    /// Declared output interpretation and the admitted plan for it.
    context: Arc<ColorContext>,
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
        let source = PixelSlice::new(
            rows.input.as_slice(),
            rows.width,
            self.pending,
            rows.source_stride,
            rows.source,
        )?
        .with_color_context(rows.current.clone());
        let destination = PixelSliceMut::new(
            rows.output.as_mut_slice(),
            rows.width,
            self.pending,
            rows.output_stride,
            rows.conversion.output_descriptor(),
        )?;
        let converted = rows.conversion.convert_rows(source, destination)?;
        if converted.color_context().map(|c| **c != *self.context) != Some(false) {
            return Err(Refusal::OutputColor.into());
        }
        self.encoder.push_rows(converted.as_pixel_slice())?;
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
        context: Arc<ColorContext>,
        plan: EncodePlan,
    ) -> Result<(), Cause> {
        if self.0.is_some() {
            return Err(Refusal::RepeatedDestination.into());
        }
        self.0 = Some(Running {
            rows,
            encoder,
            context,
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
        descriptor: PixelDescriptor,
    ) -> Result<(), SinkError> {
        let run = self.0.as_mut().ok_or(Refusal::DecoderBeforeSelection)?;
        if run.begun
            || (width, height, descriptor) != (run.rows.width, run.rows.height, run.rows.source)
        {
            return Err(Refusal::GeometryOrOrientation.into());
        }
        // Start exactly the admitted plan, with the rows' color declared
        // before the header exists; a changed startup refuses before output.
        let declaration = EncodeDeclaration::new(width, height, run.rows.output_descriptor())
            .with_color(&run.context);
        let observed = run.encoder.begin_declared(&declaration)?;
        run.plan.validate_started(&observed)?;
        run.begun = true;
        Ok(())
    }
    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, SinkError> {
        let run = self.0.as_mut().ok_or(Refusal::DecoderBeforeSelection)?;
        if !run.begun
            || run.finished
            || height == 0
            || height > run.rows.max_rows
            || width != run.rows.width
            || descriptor != run.rows.source
            || y != run.pushed + run.pending
            || y.checked_add(height)
                .is_none_or(|end| end > run.rows.height)
        {
            return Err(Refusal::StripGeometry.into());
        }
        run.flush()?;
        run.pending = height;
        Ok(PixelSliceMut::new(
            run.rows.input.as_mut_slice(),
            width,
            height,
            run.rows.source_stride,
            descriptor,
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
