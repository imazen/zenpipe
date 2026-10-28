//! A controlled decoder served as a pull-based pipeline source.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use whereat::at;
use zencodec::decode::{
    BoundaryAction, BoundaryId, ControlledIncrementalDecode, ControlledProgress, DecodeBoundary,
    DecodeRowSink, DynControlledDecoder, ErasedError, Event, Fused, IncrementalDecode, Input,
    InputRequest, InterpretationState, SinkError, StepError,
};
use zencodec::operation::{
    DecodeRequest, OperationPolicy, OperationReport, PolicySet, resolve_boundary,
};
use zencodec::source::{ReadAt, SliceReadAt};
use zencodec::storage::{OwnedStorage, StorageBudget};
use zencodec::{CodecSet, ImageInfo, SharedPulse};
use zencodec_io::{ByteStepper, DriveOptions, DriveStats, ReadAtCursor, Step, Stepped, Yield};
use zenpixels::{ColorContext, PixelDescriptor, PixelSlice, PixelSliceMut};

use super::{Refusal, SharedBytes};
use crate::error::{PipeError, PipeResult};
use crate::format::PixelFormat;
use crate::strip::Strip;

fn refused(refusal: Refusal) -> whereat::At<PipeError> {
    at!(PipeError::Codec(Box::new(refusal)))
}

fn failed(error: impl core::error::Error + Send + Sync + 'static) -> whereat::At<PipeError> {
    at!(PipeError::Codec(Box::new(error)))
}

/// Admission, queue and control settings for [`DecodeSource`].
#[derive(Clone)]
pub struct DecodeOptions {
    storage: StorageBudget,
    strip_rows: u32,
    slots: u32,
    drive: DriveOptions,
    pulse: SharedPulse,
}

impl DecodeOptions {
    /// Share `storage` between the controlled decoder (its retained input and
    /// delivery rows) and this adapter's strip slots. Defaults: 64-row strips,
    /// two slots, default drive options and no pulse. The operation policy is
    /// an argument of [`DecodeSource::new`], not an option.
    pub fn new(storage: StorageBudget) -> Self {
        Self {
            storage,
            strip_rows: 64,
            slots: 2,
            drive: DriveOptions::default(),
            pulse: SharedPulse::default(),
        }
    }

    /// Tallest strip lent to the decoder. Native batches taller than this are
    /// divided by the decoder's delivery; every piece needs a free slot.
    pub fn with_strip_rows(mut self, rows: u32) -> Self {
        self.strip_rows = rows;
        self
    }

    /// Strip slots, each admitted from the shared storage when first needed.
    /// A step that delivers more strips than this refuses instead of growing.
    pub fn with_queue_slots(mut self, slots: u32) -> Self {
        self.slots = slots;
        self
    }

    /// Read sizes, range coalescing and input admission for the source.
    pub fn with_drive(mut self, drive: DriveOptions) -> Self {
        self.drive = drive;
        self
    }

    /// Cancellation and progress for every step and decision. Pass the same
    /// pulse the codec set stamps on its jobs; the set does not lend an
    /// owned copy.
    pub fn with_pulse(mut self, pulse: SharedPulse) -> Self {
        self.pulse = pulse;
        self
    }
}

/// What the adapter and its driver actually did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeStats {
    /// Driver counters: steps, source reads, bytes read, rows reported.
    pub drive: DriveStats,
    /// Strips the decoder delivered.
    pub strips: u64,
    /// Most strips the decoder delivered within one step.
    pub max_strips_per_step: u32,
    /// Tallest strip delivered.
    pub max_strip_rows: u32,
    /// Strip slots allocated.
    pub slots: u32,
    /// Bytes admitted for strip slots.
    pub slot_bytes: usize,
}

struct Slot {
    data: OwnedStorage<u8>,
    busy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout {
    width: u32,
    height: u32,
    descriptor: PixelDescriptor,
}

/// Fixed strip slots lent to the decoder in order; never grows past its limit.
struct Queue {
    storage: StorageBudget,
    strip_rows: u32,
    max_slots: u32,
    slots: Vec<Slot>,
    /// Declared by the final interpretation, before the decoder's `begin`.
    declared: Option<Layout>,
    began: Option<(Layout, usize)>,
    ready: VecDeque<(usize, u32)>,
    lent: Option<(usize, u32)>,
    next_y: u32,
    finished: bool,
    step_strips: u32,
    stats: DecodeStats,
}

impl Queue {
    /// A lent strip is complete once the decoder asks for the next one, calls
    /// `finish`, or returns from the step (it cannot keep the borrow).
    fn settle(&mut self) {
        if let Some(lent) = self.lent.take() {
            self.ready.push_back(lent);
        }
    }

    fn free_slot(&mut self, stride: usize) -> Result<usize, SinkError> {
        if let Some(i) = self.slots.iter().position(|s| !s.busy) {
            return Ok(i);
        }
        if self.slots.len() as u32 >= self.max_slots {
            return Err(Refusal::StripQueueFull {
                slots: self.max_slots,
            }
            .into());
        }
        let len = stride
            .checked_mul(self.strip_rows as usize)
            .ok_or(Refusal::StripGeometry)?;
        self.slots.try_reserve(1)?;
        let data = self.storage.bytes(len)?;
        self.slots.push(Slot { data, busy: false });
        self.stats.slots += 1;
        self.stats.slot_bytes += len;
        Ok(self.slots.len() - 1)
    }
}

impl DecodeRowSink for Queue {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), SinkError> {
        let layout = Layout {
            width,
            height,
            descriptor,
        };
        if self.began.is_some()
            || width == 0
            || height == 0
            || self.declared.is_some_and(|declared| declared != layout)
        {
            return Err(Refusal::StripGeometry.into());
        }
        let stride = (width as usize)
            .checked_mul(descriptor.bytes_per_pixel())
            .ok_or(Refusal::StripGeometry)?;
        self.began = Some((layout, stride));
        Ok(())
    }

    fn max_buffer_rows(&self) -> NonZeroU32 {
        NonZeroU32::new(self.strip_rows).unwrap_or(NonZeroU32::MIN)
    }

    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, SinkError> {
        let (layout, stride) = self.began.ok_or(Refusal::MissingDescriptor)?;
        if self.finished
            || width != layout.width
            || descriptor != layout.descriptor
            || height == 0
            || y != self.next_y
            || y.checked_add(height).is_none_or(|end| end > layout.height)
        {
            return Err(Refusal::StripGeometry.into());
        }
        if height > self.strip_rows {
            return Err(Refusal::StripTooTall {
                rows: height,
                max: self.strip_rows,
            }
            .into());
        }
        self.settle();
        let slot = self.free_slot(stride)?;
        self.slots[slot].busy = true;
        self.lent = Some((slot, height));
        self.next_y = y + height;
        self.step_strips += 1;
        self.stats.strips += 1;
        self.stats.max_strip_rows = self.stats.max_strip_rows.max(height);
        let len = stride * height as usize;
        Ok(PixelSliceMut::new(
            &mut self.slots[slot].data.as_mut_slice()[..len],
            width,
            height,
            stride,
            descriptor,
        )?)
    }

    fn finish(&mut self) -> Result<(), SinkError> {
        let (layout, _) = self.began.ok_or(Refusal::MissingDescriptor)?;
        self.settle();
        if self.finished || self.next_y != layout.height {
            return Err(Refusal::MissingRows.into());
        }
        self.finished = true;
        Ok(())
    }
}

/// What one productive step of the held decoder produced.
enum Produced {
    /// Rows, metadata or completion; the queue holds any delivered strips.
    Decode,
    Suspended(DecodeBoundary),
}

/// The native session and its strip queue, stepped through any byte cursor.
/// A native, sink or cancellation failure drops the session.
struct Stage {
    decoder: Option<Fused<Box<DynControlledDecoder>>>,
    queue: Queue,
}

impl Stage {
    fn resume(
        &mut self,
        boundary: BoundaryId,
        action: BoundaryAction,
        pulse: &dyn zencodec::Pulse,
    ) -> Result<(), StepError<ErasedError>> {
        let mut decoder = self.decoder.take().ok_or(StepError::Finished)?;
        decoder.resume(boundary, action, pulse)?;
        self.decoder = Some(decoder);
        Ok(())
    }

    fn info(&self) -> Option<&ImageInfo> {
        self.decoder.as_ref().and_then(|d| d.info())
    }
}

impl ByteStepper for Stage {
    type Output = Produced;
    type Error = StepError<ErasedError>;

    fn step(
        &mut self,
        input: Input<'_>,
        pulse: &dyn zencodec::Pulse,
    ) -> Result<Stepped<Produced>, StepError<ErasedError>> {
        let mut decoder = self.decoder.take().ok_or(StepError::Finished)?;
        self.queue.step_strips = 0;
        let result = decoder.step_controlled(input, &mut self.queue, pulse);
        self.queue.settle();
        let stats = &mut self.queue.stats;
        stats.max_strips_per_step = stats.max_strips_per_step.max(self.queue.step_strips);
        let (consumed, step) = match result? {
            ControlledProgress::Suspended { consumed, boundary } => {
                (consumed, Step::Output(Produced::Suspended(boundary)))
            }
            ControlledProgress::Decode(progress) => {
                let step = match progress.event {
                    Event::Need => Step::Need,
                    Event::Done(_) => Step::Finished(Produced::Decode),
                    _ => Step::Output(Produced::Decode),
                };
                (progress.consumed, step)
            }
        };
        self.decoder = Some(decoder);
        Ok(Stepped { consumed, step })
    }

    fn requested(&self) -> &[InputRequest] {
        self.decoder.as_ref().map_or(&[], |d| d.requested())
    }
}

/// A controlled decoder over a positional source, served one strip per pull.
///
/// Construction reads until the decoder states its final interpretation,
/// answers each mandatory decode question through its operation policy, and stops
/// there: `width`, `height`, `format` and [`info`](Self::info) are final, but
/// no pixel row is decoded until the first [`next`](crate::Source::next).
/// An encode refusal that happens between construction and the first pull
/// therefore costs no pixel decode. Each pull steps the decoder only until
/// one strip is ready.
///
/// Slices take the copy-free path through [`SliceReadAt`]. A file or network
/// source reads through [`ReadAtCursor`] with the configured
/// [`DriveOptions`]: sequential requests read `read_size` ahead; independent
/// ranges are coalesced over `max_gap` into one `read_many_at` round.
/// Decoders that retain their whole compressed input before their final
/// interpretation (the current JPEG and PNG routes) read all of it during
/// construction.
pub struct DecodeSource<R, P = PolicySet> {
    stage: Stage,
    source: R,
    cursor: ReadAtCursor,
    policy: P,
    report: OperationReport,
    pulse: SharedPulse,
    /// The adopted final interpretation, applied at the first pull.
    pending: Option<(BoundaryId, BoundaryAction)>,
    info: ImageInfo,
    layout: Layout,
    color: Arc<ColorContext>,
    served: Option<usize>,
    done: bool,
    delivered: u32,
}

impl<'a> DecodeSource<SliceReadAt<'a>, PolicySet> {
    /// Decode resident bytes with the default request and an empty
    /// [`PolicySet`]. [`new`](DecodeSource::new) takes any other policy.
    pub fn from_slice(
        codecs: &CodecSet,
        bytes: &'a [u8],
        options: DecodeOptions,
    ) -> PipeResult<Self> {
        Self::new(
            codecs,
            &DecodeRequest::new(),
            SliceReadAt::new(bytes),
            PolicySet::new(),
            options,
        )
    }
}

impl DecodeSource<SharedBytes, PolicySet> {
    /// Decode owned resident bytes with the default request and an empty
    /// [`PolicySet`]. The source is `'static`, so it can feed operators that box
    /// their upstream (crop, resize, filters). The bytes are not copied, and
    /// they stay resident until this source is dropped: see [`SharedBytes`].
    pub fn from_bytes(
        codecs: &CodecSet,
        bytes: impl Into<SharedBytes>,
        options: DecodeOptions,
    ) -> PipeResult<Self> {
        Self::new(
            codecs,
            &DecodeRequest::new(),
            bytes.into(),
            PolicySet::new(),
            options,
        )
    }
}

impl<R: ReadAt, P: OperationPolicy> DecodeSource<R, P> {
    /// Start a controlled decode of `source` and drive it to its final
    /// interpretation. `policy` answers each mandatory decode question. Pass a
    /// [`PolicySet`], a closure, or `BoundPolicy::new(requirements, resolver)`.
    /// The requirements of a set or of a binding are checked before the
    /// callback runs, and a refusal names the unmet ones.
    pub fn new(
        codecs: &CodecSet,
        request: &DecodeRequest,
        source: R,
        policy: P,
        options: DecodeOptions,
    ) -> PipeResult<Self> {
        if options.strip_rows == 0 || options.slots == 0 {
            return Err(refused(Refusal::InvalidOptions));
        }
        let decoder = request
            .start_controlled(codecs, options.storage.clone())
            .map_err(failed)?;
        let queue = Queue {
            storage: options.storage,
            strip_rows: options.strip_rows,
            max_slots: options.slots,
            slots: Vec::new(),
            declared: None,
            began: None,
            ready: VecDeque::new(),
            lent: None,
            next_y: 0,
            finished: false,
            step_strips: 0,
            stats: DecodeStats::default(),
        };
        let mut this = Self {
            stage: Stage {
                decoder: Some(Fused::new(decoder)),
                queue,
            },
            source,
            cursor: ReadAtCursor::new(options.drive),
            policy,
            report: OperationReport::default(),
            pulse: options.pulse,
            pending: None,
            info: ImageInfo::new(0, 0, zencodec::ImageFormat::Unknown),
            layout: Layout {
                width: 0,
                height: 0,
                descriptor: PixelDescriptor::RGB8_SRGB,
            },
            color: Arc::new(ColorContext::default()),
            served: None,
            done: false,
            delivered: 0,
        };
        this.prime()?;
        Ok(this)
    }

    /// Step until the final interpretation is decided (held, not adopted) or
    /// the decoder declares its output by delivering rows.
    fn prime(&mut self) -> PipeResult<()> {
        loop {
            let produced = self
                .cursor
                .next(&mut self.source, &mut self.stage, self.pulse.as_pulse())
                .map_err(failed)?;
            match produced {
                Yield::Output(Produced::Suspended(boundary)) => {
                    let action = self.decide(boundary)?;
                    let facts = boundary.facts();
                    if facts.state == InterpretationState::Final
                        && action == BoundaryAction::AdoptInterpretation
                        && let Some(descriptor) = facts.descriptor
                    {
                        let layout = Layout {
                            width: facts.width,
                            height: facts.height,
                            descriptor,
                        };
                        self.stage.queue.declared = Some(layout);
                        self.pending = Some((boundary.id(), action));
                        return self.declare(layout);
                    }
                    self.stage
                        .resume(boundary.id(), action, self.pulse.as_pulse())
                        .map_err(failed)?;
                }
                Yield::Output(Produced::Decode) => {
                    if let Some((layout, _)) = self.stage.queue.began {
                        return self.declare(layout);
                    }
                }
                Yield::Finished(_) => {
                    self.done = true;
                    let (layout, _) = self
                        .stage
                        .queue
                        .began
                        .ok_or_else(|| refused(Refusal::MissingDescriptor))?;
                    return self.declare(layout);
                }
            }
        }
    }

    fn declare(&mut self, layout: Layout) -> PipeResult<()> {
        let info = self
            .stage
            .info()
            .cloned()
            .ok_or_else(|| refused(Refusal::MissingDescriptor))?;
        self.color = Arc::new(info.source_color.to_color_context());
        self.info = info;
        self.layout = layout;
        Ok(())
    }

    fn decide(&mut self, boundary: DecodeBoundary) -> PipeResult<BoundaryAction> {
        match resolve_boundary(
            boundary,
            &mut self.policy,
            // No legacy preset: the policy carries its own requirements.
            PolicySet::new(),
            self.pulse.as_pulse(),
        ) {
            Ok((action, record)) => {
                self.report.record(record);
                Ok(action)
            }
            Err(error) => {
                self.report.record(error.error().record());
                Err(failed(error))
            }
        }
    }

    /// Image facts at the final interpretation: orientation, color carriers,
    /// EXIF/XMP and supplements.
    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    /// Decisions made at decode suspensions so far.
    pub fn report(&self) -> &OperationReport {
        &self.report
    }

    /// Driver and queue counters so far.
    pub fn stats(&self) -> DecodeStats {
        let mut stats = self.stage.queue.stats;
        stats.drive = self.cursor.stats();
        stats
    }

    /// Driver-owned input capacity currently retained (window and fetched
    /// ranges). Borrowed resident bytes and decoder-retained input are excluded.
    pub fn retained_input_capacity(&self) -> usize {
        self.cursor.retained_input_capacity()
    }

    fn pull(&mut self) -> PipeResult<Option<(usize, u32)>> {
        if let Some((boundary, action)) = self.pending.take() {
            self.stage
                .resume(boundary, action, self.pulse.as_pulse())
                .map_err(failed)?;
        }
        loop {
            if let Some(ready) = self.stage.queue.ready.pop_front() {
                return Ok(Some(ready));
            }
            if self.done {
                if self.delivered != self.layout.height {
                    return Err(refused(Refusal::MissingRows));
                }
                return Ok(None);
            }
            let produced = self
                .cursor
                .next(&mut self.source, &mut self.stage, self.pulse.as_pulse())
                .map_err(failed)?;
            match produced {
                Yield::Output(Produced::Suspended(boundary)) => {
                    let action = self.decide(boundary)?;
                    self.stage
                        .resume(boundary.id(), action, self.pulse.as_pulse())
                        .map_err(failed)?;
                }
                Yield::Output(Produced::Decode) => {}
                Yield::Finished(_) => self.done = true,
            }
        }
    }
}

impl<R: ReadAt + Send, P: OperationPolicy + Send> crate::Source for DecodeSource<R, P> {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        if let Some(slot) = self.served.take() {
            self.stage.queue.slots[slot].busy = false;
        }
        let Some((slot, rows)) = self.pull()? else {
            return Ok(None);
        };
        let (layout, stride) = self
            .stage
            .queue
            .began
            .ok_or_else(|| refused(Refusal::MissingDescriptor))?;
        if layout != self.layout {
            return Err(refused(Refusal::StripGeometry));
        }
        self.served = Some(slot);
        self.delivered += rows;
        let strip = PixelSlice::new(
            &self.stage.queue.slots[slot].data.as_slice()[..stride * rows as usize],
            layout.width,
            rows,
            stride,
            layout.descriptor,
        )
        .map_err(failed)?
        .with_color_context(self.color.clone());
        Ok(Some(strip))
    }

    fn width(&self) -> u32 {
        self.layout.width
    }

    fn height(&self) -> u32 {
        self.layout.height
    }

    fn format(&self) -> PixelFormat {
        self.layout.descriptor
    }

    /// The decoder's current interpretation, attached to every strip.
    fn color_context(&self) -> Option<Arc<ColorContext>> {
        Some(self.color.clone())
    }
}
