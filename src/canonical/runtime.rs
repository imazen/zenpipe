//! Unified canonical `PipelineRuntime` executing validated 10-node [`Graph`](crate::canonical::Graph)s
//! over one `Arc<zencodecs::Engine>` and explicit [`HostServices`].
//!
//! Supports:
//! - Still & animated image DAGs (including Display P3 / BT.2020 `zenfilters::recipe`,
//!   full `zenlayout::Layout` crop + resize + pad execution, W3C Porter-Duff linear
//!   compositing, `Preserve` vs `PreserveBytes`, `Coding::Verify`, and provider swapping
//!   via `Routing`),
//! - Video -> still poster extraction by presentation index or rational presentation timestamp
//!   (MP4, WebM, IVF × AV1, VP8, VP9),
//! - Video -> video streaming transcode (`MediaPump` when pixel-untouched, or per-frame
//!   geometry/filter -> `encode_frames_to_video` when transformed, preserving track & timing
//!   policies),
//! - Encoded & raw Animation -> video (enforcing explicit `LoopPolicy` & `TimingPolicy`),
//! - Video -> animation (enforcing explicit `TrackPolicy`, `LoopPolicy`, `TimingPolicy`, and
//!   workspace admission).

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use whereat::at;
use zencodec::encode::Fidelity;
#[cfg(feature = "media")]
use zencodec::time::Timestamp;
use zencodec::time::{FrameDuration, TimeBase};
use zencodec::{ImageEncodeRole, ImageFormat, Metadata, ResourceLimits, SharedPulse};
use zencodecs::Stop;
#[cfg(feature = "media")]
use zencodecs::engine::TargetFormat;
use zencodecs::engine::{
    Coding, DecodeNeeds, Engine, EngineError, LoopCount, LoopPolicy, MediaKind, Metric,
    MetricTarget, QualityLevel, RouteReport, Run, SequencePolicy, TimingPolicy, WorkspacePolicy,
};
use zenlayout::{CanvasColor, Orientation, OrientationExt, Rect, Size};
use zenpixels::{
    AlphaMode, ColorSpace, DiffuseWhite, PixelBuf, PixelFormat, PixelView, Primaries, SampleLayout,
    SignalRange, Transfer,
};
use zenpixels_convert::{ColorServices, ConvertPolicy, RowPlan};
use zenresize::{Filter as ResizeFilter, ResizeConfig, Resizer};

#[cfg(feature = "media")]
use zencodec_media::registry::ContainerId;
#[cfg(feature = "media")]
use zencodec_media::track::{CodecId as MediaCodecId, TrackInfo};
#[cfg(feature = "media")]
use zencodecs::media::pump::{
    encode_frames_to_video, extract_poster_from_bytes, transcode_media_bytes,
};
#[cfg(feature = "media")]
use zencodecs::media::route::validate_track_policy;
#[cfg(feature = "media")]
use zencodecs::media::select::{
    DecodeSelection, PresentationEntry, build_presentation_index, select_presentation_index,
};
#[cfg(feature = "media")]
use zencodecs::media::timeline::quantize_endpoints;

use crate::canonical::graph::{
    BlendKind, CanonicalJob, ColorIngestMode, CompositeOp, DecodeNodeOp, EncodeNodeOp,
    FilterNodeOp, GeometryOp, Graph, ImageOp, OutputOp, ResizeKernel, SlotName,
};
#[cfg(feature = "canonical-json")]
use crate::canonical::json::OutputV2;
use crate::error::{PipeError, PipeResult};

/// Host-supplied runtime policies and services attached to [`PipelineRuntime`].
#[derive(Clone, Default)]
pub struct HostServices {
    /// Ceiling resource limits (job limits may only tighten these).
    pub limits: Option<ResourceLimits>,
    /// Native workspace accounting policy (defaults to [`WorkspacePolicy::Strict`]).
    pub workspace_policy: WorkspacePolicy,
    /// Optional provider routing policy layered beneath `Engine::routing`.
    pub routing: Option<zencodecs::routing::Routing>,
    /// Optional per-runtime color services override.
    pub color_services: Option<ColorServices>,
    /// Optional bounded route trace capacity.
    pub trace_capacity: Option<NonZeroU32>,
}

impl HostServices {
    /// Construct strict default host services (`WorkspacePolicy::Strict`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set ceiling [`ResourceLimits`].
    #[must_use]
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set [`WorkspacePolicy`] (`Strict` default; `BestEffort` opts into unknown native bounds).
    #[must_use]
    pub const fn with_workspace_policy(mut self, policy: WorkspacePolicy) -> Self {
        self.workspace_policy = policy;
        self
    }

    /// Set provider [`Routing`](zencodecs::routing::Routing).
    #[must_use]
    pub fn with_routing(mut self, routing: zencodecs::routing::Routing) -> Self {
        self.routing = Some(routing);
        self
    }

    /// Set explicit [`ColorServices`] override.
    #[must_use]
    pub fn with_color_services(mut self, services: ColorServices) -> Self {
        self.color_services = Some(services);
        self
    }

    /// Enable bounded route tracing with `capacity` events.
    #[must_use]
    pub const fn with_trace(mut self, capacity: NonZeroU32) -> Self {
        self.trace_capacity = Some(capacity);
        self
    }

    fn effective_limits(&self, engine: &Engine, job: &CanonicalJob) -> ResourceLimits {
        let mut lim = *engine.limits();
        if let Some(host) = self.limits {
            lim.max_width = min_opt_u32(lim.max_width, host.max_width);
            lim.max_height = min_opt_u32(lim.max_height, host.max_height);
            lim.max_pixels = min_opt_u64(lim.max_pixels, host.max_pixels);
            lim.max_input_bytes = min_opt_u64(lim.max_input_bytes, host.max_input_bytes);
            lim.max_output_bytes = min_opt_u64(lim.max_output_bytes, host.max_output_bytes);
            lim.max_memory_bytes = min_opt_u64(lim.max_memory_bytes, host.max_memory_bytes);
            lim.max_frames = min_opt_u32(lim.max_frames, host.max_frames);
        }
        if let Some(max_out) = job.max_output_bytes {
            lim.max_output_bytes = min_opt_u64(lim.max_output_bytes, Some(max_out));
        }
        if let Some(max_dim) = job.max_dimension {
            lim.max_width = min_opt_u32(lim.max_width, Some(max_dim));
            lim.max_height = min_opt_u32(lim.max_height, Some(max_dim));
        }
        lim
    }

    fn build_run_with_pulse<'a>(
        &'a self,
        engine: &Engine,
        job: &CanonicalJob,
        pulse: SharedPulse,
    ) -> Run<'a> {
        let lim = self.effective_limits(engine, job);
        self.build_run_with_limits_and_pulse(lim, pulse)
    }

    fn build_run_with_limits_and_pulse<'a>(
        &'a self,
        lim: ResourceLimits,
        pulse: SharedPulse,
    ) -> Run<'a> {
        let mut run = Run::new()
            .with_workspace_policy(self.workspace_policy)
            .with_limits(lim)
            .with_pulse(pulse);
        if let Some(ref routing) = self.routing {
            run = run.with_routing(routing);
        }
        if let Some(ref cs) = self.color_services {
            run = run.with_color_services(cs.clone());
        }
        if let Some(cap) = self.trace_capacity {
            run = run.with_trace(cap);
        }
        run
    }
}

fn min_opt_u32(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn min_opt_u64(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// One bound input slot supplied to [`PipelineRuntime::prepare`].
#[derive(Clone)]
pub enum InputBinding<'io> {
    /// Encoded image, animation, or video container bytes.
    Bytes(&'io [u8]),
    /// Raw decoded still pixel view.
    Pixels(PixelView<'io>),
    /// Sequence of raw frames with exact rational durations and loop count.
    Frames {
        /// Ordered presentation frames.
        frames: Vec<(PixelView<'io>, FrameDuration)>,
        /// Source loop behavior.
        loops: LoopCount,
    },
}

/// Named input slot bindings for one canonical job execution.
#[derive(Clone, Default)]
pub struct Bindings<'io> {
    inputs: BTreeMap<SlotName, InputBinding<'io>>,
}

impl<'io> Bindings<'io> {
    /// Construct empty bindings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn insert_unique(mut self, slot: &str, binding: InputBinding<'io>) -> PipeResult<Self> {
        let key = SlotName::new(slot).map_err(|e| at!(PipeError::Op(e.to_string())))?;
        if self.inputs.contains_key(&key) {
            return Err(at!(PipeError::Op(format!(
                "duplicate input binding for slot '{}'",
                key.as_str()
            ))));
        }
        self.inputs.insert(key, binding);
        Ok(self)
    }

    /// Bind encoded bytes to `slot` (rejects duplicate bindings for the same slot).
    pub fn with_bytes(self, slot: &str, bytes: &'io [u8]) -> PipeResult<Self> {
        self.insert_unique(slot, InputBinding::Bytes(bytes))
    }

    /// Bind raw pixels to `slot` (rejects duplicate bindings for the same slot).
    pub fn with_pixels(self, slot: &str, pixels: PixelView<'io>) -> PipeResult<Self> {
        self.insert_unique(slot, InputBinding::Pixels(pixels))
    }

    /// Bind an in-memory sequence of frames with rational durations to `slot`.
    pub fn with_frames(
        self,
        slot: &str,
        frames: Vec<(PixelView<'io>, FrameDuration)>,
        loops: LoopCount,
    ) -> PipeResult<Self> {
        self.insert_unique(slot, InputBinding::Frames { frames, loops })
    }
}

/// Pre-execution binding and static-limit summary returned by [`PreparedJob::admission`].
///
/// Summarizes graph shape, bound slots, and input footprints validated at [`PipelineRuntime::prepare`].
/// Provider workspace bounds and dynamic pixel/output limits are enforced during [`PreparedJob::run`].
#[derive(Clone, Debug)]
pub struct AdmissionReport {
    /// Total image nodes in the validated graph.
    pub node_count: usize,
    /// Total terminal outputs in the validated graph.
    pub output_count: usize,
    /// Bound input slot names.
    pub input_slots: Vec<SlotName>,
    /// Declared output slot names.
    pub output_slots: Vec<SlotName>,
    /// Conservative input byte footprint.
    pub input_bytes: u64,
    /// Non-fatal planning advisories.
    pub warnings: Vec<String>,
}

/// One completed output artifact produced by [`PreparedJob::run`].
#[derive(Clone, Debug)]
pub struct OutputArtifact {
    /// Output slot name.
    pub slot: SlotName,
    /// Media kind (`"still"`, `"animation"`, `"video"`, `"pixels"`).
    pub kind: &'static str,
    /// MIME type (`"image/jpeg"`, `"video/mp4"`, `"video/webm"`, etc.).
    pub mime: String,
    /// Encoded bytes (empty for `kind == "pixels"`).
    pub bytes: Vec<u8>,
    /// Raw pixels (present when `kind == "pixels"`).
    pub pixels: Option<PixelBuf>,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Frame count if sequence/video.
    pub frames: Option<u64>,
    /// Selected video codec token (`"av1"`, `"vp8"`, `"vp9"`) when `kind == "video"`.
    pub video_codec: Option<String>,
    /// Full provider/color/workspace route report from `zencodecs::Engine`.
    pub report: RouteReport,
}

/// Aggregated outputs and warnings from one canonical job run.
#[derive(Clone, Debug)]
pub struct JobResult {
    /// Completed output artifacts in graph declaration order.
    pub outputs: Vec<OutputArtifact>,
    /// Non-fatal diagnostics.
    pub warnings: Vec<String>,
}

impl JobResult {
    /// Look up one output artifact by slot name.
    #[must_use]
    pub fn artifact(&self, slot: &str) -> Option<&OutputArtifact> {
        self.outputs.iter().find(|a| a.slot.as_str() == slot)
    }

    /// Build the canonical [`OutputV2`] modern wire response shared with `imageflow`.
    #[cfg(feature = "canonical-json")]
    #[must_use]
    pub fn output_v2(&self) -> OutputV2 {
        let outputs = self
            .outputs
            .iter()
            .map(|a| {
                let byte_len = if let Some(ref px) = a.pixels {
                    (px.as_view().row_bytes() as u64).saturating_mul(u64::from(px.height()))
                } else {
                    a.bytes.len() as u64
                };
                OutputV2::artifact_from_report(
                    a.slot.as_str().to_string(),
                    a.kind,
                    a.mime.clone(),
                    byte_len,
                    a.width,
                    a.height,
                    a.frames,
                    a.video_codec.clone(),
                    &a.report,
                )
            })
            .collect();
        OutputV2::new(outputs, self.warnings.clone())
    }
}

/// Authoritative runtime executing canonical 10-node pipelines over one shared `Engine`.
#[derive(Clone)]
pub struct PipelineRuntime {
    engine: Arc<Engine>,
    services: HostServices,
}

impl PipelineRuntime {
    /// Construct a [`PipelineRuntime`] bound to one immutable [`Engine`] and [`HostServices`].
    #[must_use]
    pub fn new(engine: Arc<Engine>, services: HostServices) -> Self {
        Self { engine, services }
    }

    /// Borrow the underlying [`Engine`].
    #[must_use]
    pub const fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Borrow the runtime's [`HostServices`].
    #[must_use]
    pub const fn services(&self) -> &HostServices {
        &self.services
    }

    /// Validate bindings and static limits against `job`, compute an [`AdmissionReport`],
    /// and return a [`PreparedJob`].
    pub fn prepare<'io>(
        &self,
        job: &CanonicalJob,
        bindings: Bindings<'io>,
    ) -> PipeResult<PreparedJob<'io>> {
        let graph = job.graph();
        let limits = self.services.effective_limits(&self.engine, job);
        let mut input_bytes = 0u64;
        let mut input_slots = Vec::new();
        for slot in graph.input_slots() {
            let binding = bindings.inputs.get(slot).ok_or_else(|| {
                at!(PipeError::Op(format!(
                    "missing required input binding for slot '{}'",
                    slot.as_str()
                )))
            })?;
            match binding {
                InputBinding::Bytes(b) => {
                    if let Some(max_in) = limits.max_input_bytes
                        && (b.len() as u64) > max_in
                    {
                        return Err(at!(PipeError::LimitExceeded(format!(
                            "input slot '{}' length {} B exceeds max_input_bytes {max_in}",
                            slot.as_str(),
                            b.len()
                        ))));
                    }
                    input_bytes = input_bytes.saturating_add(b.len() as u64);
                }
                InputBinding::Pixels(v) => {
                    check_canvas_limits(
                        v.width(),
                        v.height(),
                        v.format().layout.bytes_per_pixel(),
                        &limits,
                    )?;
                    let sz = (v.row_bytes() as u64).saturating_mul(u64::from(v.height()));
                    input_bytes = input_bytes.saturating_add(sz);
                }
                InputBinding::Frames { frames, .. } => {
                    if let Some(max_f) = limits.max_frames
                        && frames.len() > max_f as usize
                    {
                        return Err(at!(PipeError::LimitExceeded(format!(
                            "input slot '{}' frame count {} exceeds max_frames {max_f}",
                            slot.as_str(),
                            frames.len()
                        ))));
                    }
                    for (v, _) in frames {
                        check_canvas_limits(
                            v.width(),
                            v.height(),
                            v.format().layout.bytes_per_pixel(),
                            &limits,
                        )?;
                        let sz = (v.row_bytes() as u64).saturating_mul(u64::from(v.height()));
                        input_bytes = input_bytes.saturating_add(sz);
                    }
                }
            }
            input_slots.push((*slot).clone());
        }
        let output_slots = graph.output_slots().into_iter().cloned().collect();
        let admission = AdmissionReport {
            node_count: graph.nodes().len(),
            output_count: graph.outputs().len(),
            input_slots,
            output_slots,
            input_bytes,
            warnings: job.diagnostics.clone(),
        };
        Ok(PreparedJob {
            engine: Arc::clone(&self.engine),
            services: self.services.clone(),
            job: job.clone(),
            bindings,
            admission,
        })
    }
}

/// Prepared, admitted canonical pipeline ready for execution.
pub struct PreparedJob<'io> {
    engine: Arc<Engine>,
    services: HostServices,
    job: CanonicalJob,
    bindings: Bindings<'io>,
    admission: AdmissionReport,
}

/// Cumulative live runtime pixel/workspace memory tracker enforcing `max_memory_bytes`
/// across ALL simultaneously retained node buffers and temporary operation scratch!
#[derive(Clone, Copy, Debug)]
struct LiveMemBudget {
    used: u64,
    limit: Option<u64>,
}

impl LiveMemBudget {
    const fn new(limits: &ResourceLimits) -> Self {
        Self {
            used: 0,
            limit: limits.max_memory_bytes,
        }
    }

    fn charge(&mut self, bytes: u64) -> PipeResult<()> {
        let next = self.used.checked_add(bytes).ok_or_else(|| {
            at!(PipeError::LimitExceeded(
                "cumulative runtime memory counter overflow".to_string()
            ))
        })?;
        if let Some(cap) = self.limit
            && next > cap
        {
            return Err(at!(PipeError::LimitExceeded(format!(
                "cumulative live runtime memory {next} B exceeds max_memory_bytes {cap}"
            ))));
        }
        self.used = next;
        Ok(())
    }

    fn check_transient(&self, extra_bytes: u64) -> PipeResult<()> {
        let peak = self.used.checked_add(extra_bytes).ok_or_else(|| {
            at!(PipeError::LimitExceeded(
                "peak transient runtime memory counter overflow".to_string()
            ))
        })?;
        if let Some(cap) = self.limit
            && peak > cap
        {
            return Err(at!(PipeError::LimitExceeded(format!(
                "peak runtime memory {peak} B exceeds max_memory_bytes {cap}"
            ))));
        }
        Ok(())
    }

    fn release(&mut self, bytes: u64) {
        self.used = self.used.saturating_sub(bytes);
    }

    fn remaining(&self) -> Option<u64> {
        self.limit.map(|cap| cap.saturating_sub(self.used))
    }

    fn limits_with_remaining(&self, base: &ResourceLimits) -> ResourceLimits {
        let mut lim = *base;
        if let Some(rem) = self.remaining() {
            lim.max_memory_bytes = Some(rem);
        }
        lim
    }
}

enum NodeValue<'io> {
    Still {
        pixels: PixelBuf,
        metadata: Metadata,
        source_format: Option<ImageFormat>,
        raw_bytes: Option<&'io [u8]>,
    },
    Sequence {
        frames: Vec<(PixelBuf, Option<FrameDuration>)>,
        loops: LoopCount,
        metadata: Metadata,
        source_format: Option<ImageFormat>,
        #[cfg(feature = "media")]
        source_tracks: Option<Vec<TrackInfo>>,
        #[cfg(feature = "media")]
        source_video_track_idx: Option<usize>,
    },
    #[cfg(feature = "media")]
    VideoStream {
        bytes: &'io [u8],
        #[allow(dead_code)]
        container: ContainerId,
    },
}

fn check_canvas_limits(w: u32, h: u32, bpp: usize, limits: &ResourceLimits) -> PipeResult<usize> {
    if w == 0 || h == 0 {
        return Err(at!(PipeError::DimensionMismatch(
            "image dimensions must be non-zero".to_string()
        )));
    }
    if let Some(mw) = limits.max_width
        && w > mw
    {
        return Err(at!(PipeError::LimitExceeded(format!(
            "width {w} exceeds max_width {mw}"
        ))));
    }
    if let Some(mh) = limits.max_height
        && h > mh
    {
        return Err(at!(PipeError::LimitExceeded(format!(
            "height {h} exceeds max_height {mh}"
        ))));
    }
    let pixels = u64::from(w).saturating_mul(u64::from(h));
    if let Some(mp) = limits.max_pixels
        && pixels > mp
    {
        return Err(at!(PipeError::LimitExceeded(format!(
            "pixel count {pixels} exceeds max_pixels {mp}"
        ))));
    }
    let stride = (w as usize).checked_mul(bpp).ok_or_else(|| {
        at!(PipeError::LimitExceeded(
            "row byte stride overflow".to_string()
        ))
    })?;
    let total = stride.checked_mul(h as usize).ok_or_else(|| {
        at!(PipeError::LimitExceeded(
            "pixel buffer byte size overflow".to_string()
        ))
    })?;
    if let Some(max_mem) = limits.max_memory_bytes
        && (total as u64) > max_mem
    {
        return Err(at!(PipeError::LimitExceeded(format!(
            "buffer allocation {total} B exceeds max_memory_bytes {max_mem}"
        ))));
    }
    Ok(total)
}

fn alloc_zeroed_checked(
    total: usize,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<Vec<u8>> {
    if let Some(max_mem) = limits.max_memory_bytes
        && (total as u64) > max_mem
    {
        return Err(at!(PipeError::LimitExceeded(format!(
            "allocation {total} B exceeds max_memory_bytes {max_mem}"
        ))));
    }
    mem.charge(total as u64)?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(total).map_err(|_| {
        at!(PipeError::LimitExceeded(
            "fallible buffer allocation failed".to_string()
        ))
    })?;
    buf.resize(total, 0u8);
    Ok(buf)
}

impl<'io> PreparedJob<'io> {
    /// Inspect pre-execution admission summary.
    #[must_use]
    pub const fn admission(&self) -> &AdmissionReport {
        &self.admission
    }

    /// Execute the prepared canonical DAG and collect all declared output artifacts.
    pub fn run(self) -> PipeResult<JobResult> {
        self.run_with_pulse(SharedPulse::default())
    }

    /// Execute the prepared canonical DAG with cooperative cancellation and deadline
    /// polling (`pulse`) checked before every graph node, before every output encode,
    /// and forwarded into every codec, conversion, and `Coding::Verify` scorer call.
    pub fn run_with_pulse(self, pulse: SharedPulse) -> PipeResult<JobResult> {
        pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
        let limits = self.services.effective_limits(&self.engine, &self.job);
        let mut mem = LiveMemBudget::new(&limits);
        let run = self
            .services
            .build_run_with_pulse(&self.engine, &self.job, pulse.clone());
        let cs = self
            .services
            .color_services
            .clone()
            .unwrap_or_else(|| self.engine.color_services().clone());
        let graph: &Graph = self.job.graph();
        let mut values: Vec<NodeValue<'io>> = Vec::with_capacity(graph.nodes().len());
        let warnings = self.admission.warnings.clone();

        for op in graph.nodes() {
            pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
            let val = match op {
                ImageOp::Decode(d) => self.exec_decode(d, &run, &limits, &mut mem)?,
                ImageOp::Canvas { size, fill_rgba } => {
                    let pixels = make_solid_canvas(*size, *fill_rgba, &limits, &mut mem)?;
                    NodeValue::Still {
                        pixels,
                        metadata: Metadata::none(),
                        source_format: None,
                        raw_bytes: None,
                    }
                }
                ImageOp::ColorIngest(c) => {
                    let src = &values[c.src.index()];
                    exec_color_ingest(
                        src,
                        c.mode,
                        c.target.as_ref(),
                        c.strict_invalid_icc,
                        &cs,
                        &self.engine,
                        &run,
                        &limits,
                        &mut mem,
                    )?
                }
                ImageOp::ColorSpace(c) => {
                    let src = &values[c.src.index()];
                    exec_color_space(src, &c.to, &cs, &self.engine, &run, &limits, &mut mem)?
                }
                ImageOp::ToneMap(tm) => {
                    let src = &values[tm.src.index()];
                    exec_tone_map(src, &tm.render, &cs, &self.engine, &run, &limits, &mut mem)?
                }
                ImageOp::GainMap(gm) => {
                    let src = &values[gm.src.index()];
                    match gm.action {
                        zencodecs::engine::GainMapAction::BaseOnly => {
                            clone_node_value(src, &mut mem)?
                        }
                        _ => {
                            return Err(at!(PipeError::Op(
                                "gain_map reconstruct/carry requires a decoded gain-map attachment (not available in this runtime path)"
                                    .to_string(),
                            )));
                        }
                    }
                }
                ImageOp::MakeGainMap(_) => {
                    return Err(at!(PipeError::Op(
                        "make_gain_map derivation and container attachment is not yet supported by canonical runtime"
                            .to_string(),
                    )));
                }
                ImageOp::Geometry(g) => {
                    let src_idx = geometry_src(g).index();
                    exec_geometry(
                        &values[src_idx],
                        g,
                        &cs,
                        &self.engine,
                        &run,
                        &limits,
                        &mut mem,
                    )?
                }
                ImageOp::Filter(f) => {
                    let src_idx = filter_src(f).index();
                    exec_filter(
                        &values[src_idx],
                        f,
                        &cs,
                        &self.engine,
                        &run,
                        &limits,
                        &mut mem,
                    )?
                }
                ImageOp::Composite(comp) => {
                    let canvas = &values[comp.canvas.index()];
                    let fg = &values[comp.foreground.index()];
                    exec_composite(canvas, fg, comp, &cs, &self.engine, &run, &limits, &mut mem)?
                }
            };
            values.push(val);
        }

        let mut outputs = Vec::with_capacity(graph.outputs().len());
        for out in graph.outputs() {
            pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
            match out {
                OutputOp::Encode(enc) => {
                    let src = &values[enc.src.index()];
                    let artifact = self.exec_encode(src, enc, &cs, &run, &limits, &mut mem)?;
                    outputs.push(artifact);
                }
                OutputOp::Pixels { src, slot, format } => {
                    let mut buf = materialize_first_still(
                        &values[src.index()],
                        &self.engine,
                        &run,
                        &mut mem,
                    )?;
                    if let Some(target_fmt) = format
                        && buf.format() != target_fmt
                    {
                        let old_sz = (buf.as_view().row_bytes() as u64)
                            .saturating_mul(u64::from(buf.height()));
                        let converted = convert_pixel_buf(
                            &buf, target_fmt, &cs, false, &limits, &mut mem, &pulse,
                        )?;
                        drop(buf);
                        mem.release(old_sz);
                        buf = converted;
                    }
                    let total_bytes = check_canvas_limits(
                        buf.width(),
                        buf.height(),
                        buf.format().layout.bytes_per_pixel(),
                        &limits,
                    )? as u64;
                    if let Some(max_out) = limits.max_output_bytes
                        && total_bytes > max_out
                    {
                        return Err(at!(PipeError::LimitExceeded(format!(
                            "raw pixel output {total_bytes} B exceeds max_output_bytes {max_out}"
                        ))));
                    }
                    let (w, h) = (buf.width(), buf.height());
                    outputs.push(OutputArtifact {
                        slot: slot.clone(),
                        kind: "pixels",
                        mime: "application/octet-stream".to_string(),
                        bytes: Vec::new(),
                        pixels: Some(buf),
                        width: w,
                        height: h,
                        frames: None,
                        video_codec: None,
                        report: RouteReport::default(),
                    });
                }
            }
        }

        pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
        Ok(JobResult { outputs, warnings })
    }

    fn exec_decode(
        &self,
        op: &DecodeNodeOp,
        run: &Run<'_>,
        limits: &ResourceLimits,
        mem: &mut LiveMemBudget,
    ) -> PipeResult<NodeValue<'io>> {
        let binding = self.bindings.inputs.get(&op.slot).ok_or_else(|| {
            at!(PipeError::Op(format!(
                "missing binding for decode slot '{}'",
                op.slot.as_str()
            )))
        })?;
        match binding {
            InputBinding::Pixels(view) => {
                #[cfg(feature = "media")]
                if let Some(sel) = op.select {
                    match sel {
                        DecodeSelection::All => {}
                        DecodeSelection::PresentationIndex {
                            track: None,
                            index: 0,
                        } => {}
                        DecodeSelection::Page(0) => {}
                        _ => {
                            return Err(at!(PipeError::Op(
                                "decode.select requested non-zero or time-based presentation on raw single-frame PixelView input"
                                    .to_string(),
                            )));
                        }
                    }
                }
                #[cfg(not(feature = "media"))]
                if let Some(idx) = op.select_frame
                    && idx > 0
                {
                    return Err(at!(PipeError::Op(format!(
                        "select_frame {idx} exceeds single-frame PixelView input"
                    ))));
                }
                Ok(NodeValue::Still {
                    pixels: clone_pixel_view(view.clone(), limits, mem)?,
                    metadata: Metadata::none(),
                    source_format: None,
                    raw_bytes: None,
                })
            }
            InputBinding::Frames { frames, loops } => {
                if frames.is_empty() {
                    return Err(at!(PipeError::Op(
                        "frame sequence input binding must contain at least one frame".to_string(),
                    )));
                }
                #[cfg(feature = "media")]
                if let Some(sel) = op.select {
                    match sel {
                        DecodeSelection::All => {}
                        DecodeSelection::PresentationIndex { track, index } => {
                            if track.is_some() {
                                return Err(at!(PipeError::Op(
                                    "decode.select track override is not valid on raw frame sequence bindings"
                                        .to_string(),
                                )));
                            }
                            let idx = usize::try_from(index).map_err(|_| {
                                at!(PipeError::Op("presentation_index out of range".to_string()))
                            })?;
                            let (view, _) = frames.get(idx).ok_or_else(|| {
                                at!(PipeError::Op(format!(
                                    "presentation_index {idx} exceeds bound sequence length {}",
                                    frames.len()
                                )))
                            })?;
                            return Ok(NodeValue::Still {
                                pixels: clone_pixel_view(view.clone(), limits, mem)?,
                                metadata: Metadata::none(),
                                source_format: None,
                                raw_bytes: None,
                            });
                        }
                        DecodeSelection::PresentationTime { track, .. } => {
                            if track.is_some() {
                                return Err(at!(PipeError::Op(
                                    "decode.select track override is not valid on raw frame sequence bindings"
                                        .to_string(),
                                )));
                            }
                            let durs: Vec<FrameDuration> = frames.iter().map(|(_, d)| *d).collect();
                            let chosen = select_frame_by_time(&durs, sel)?;
                            return Ok(NodeValue::Still {
                                pixels: clone_pixel_view(frames[chosen].0.clone(), limits, mem)?,
                                metadata: Metadata::none(),
                                source_format: None,
                                raw_bytes: None,
                            });
                        }
                        DecodeSelection::Page(_) => {
                            return Err(at!(PipeError::Op(
                                "page selection is not valid on frame sequence inputs".to_string()
                            )));
                        }
                        _ => {}
                    }
                }
                #[cfg(not(feature = "media"))]
                if let Some(index) = op.select_frame {
                    let idx = usize::try_from(index)
                        .map_err(|_| at!(PipeError::Op("select_frame out of range".to_string())))?;
                    let (view, _) = frames.get(idx).ok_or_else(|| {
                        at!(PipeError::Op(format!(
                            "select_frame {idx} exceeds bound sequence length {}",
                            frames.len()
                        )))
                    })?;
                    return Ok(NodeValue::Still {
                        pixels: clone_pixel_view(view.clone(), limits, mem)?,
                        metadata: Metadata::none(),
                        source_format: None,
                        raw_bytes: None,
                    });
                }
                let mut owned = Vec::with_capacity(frames.len());
                for (v, d) in frames {
                    owned.push((clone_pixel_view(v.clone(), limits, mem)?, Some(*d)));
                }
                Ok(NodeValue::Sequence {
                    frames: owned,
                    loops: *loops,
                    metadata: Metadata::none(),
                    source_format: None,
                    #[cfg(feature = "media")]
                    source_tracks: None,
                    #[cfg(feature = "media")]
                    source_video_track_idx: None,
                })
            }
            InputBinding::Bytes(bytes) => {
                #[cfg(feature = "media")]
                {
                    if let Some((container, _)) = zencodecs::media::probe::probe_container_bytes(
                        self.engine.codecs(),
                        bytes,
                        &zencodec_media::registry::ProbeLimits::default(),
                    )
                    .map_err(map_engine_err)?
                    {
                        let is_still_bmff = container == ContainerId::Mp4
                            && matches!(
                                self.engine.codecs().detect(bytes),
                                Some(ImageFormat::Avif | ImageFormat::Heic)
                            );
                        if !is_still_bmff {
                            if let Some(sel) = op.select {
                                match sel {
                                    DecodeSelection::PresentationIndex { .. }
                                    | DecodeSelection::PresentationTime { .. } => {
                                        let comp = extract_poster_from_bytes(
                                            &self.engine,
                                            bytes,
                                            sel,
                                            PixelFormat::RGB8_SRGB,
                                            run,
                                        )
                                        .map_err(map_engine_err)?;
                                        let poster_bytes = check_canvas_limits(
                                            comp.output.0.width(),
                                            comp.output.0.height(),
                                            comp.output.0.format().layout.bytes_per_pixel(),
                                            limits,
                                        )?
                                            as u64;
                                        mem.charge(poster_bytes)?;
                                        return Ok(NodeValue::Still {
                                            pixels: comp.output.0,
                                            metadata: Metadata::none(),
                                            source_format: None,
                                            raw_bytes: None,
                                        });
                                    }
                                    DecodeSelection::All => {
                                        return Ok(NodeValue::VideoStream { bytes, container });
                                    }
                                    DecodeSelection::Page(_) => {
                                        return Err(at!(PipeError::Op(
                                            "page selection is not valid on video containers"
                                                .to_string(),
                                        )));
                                    }
                                    _ => return Ok(NodeValue::VideoStream { bytes, container }),
                                }
                            }
                            return Ok(NodeValue::VideoStream { bytes, container });
                        }
                    }
                }

                // Probe image bytes once so animated images (WebP/GIF/AVIF/APNG) decode ALL frames
                // instead of collapsing to 1 frame via `DecodeNeeds::primary()`!
                let probed = self.engine.probe(bytes, run).map_err(map_engine_err)?;
                if probed.inspection().envelope.kind == MediaKind::Animation {
                    let anim_run = self.services.build_run_with_limits_and_pulse(
                        mem.limits_with_remaining(limits),
                        run.pulse().clone(),
                    );
                    let (frames, loops, metadata, source_format) =
                        decode_encoded_animation(&self.engine, bytes, &anim_run, limits, mem)?;
                    #[cfg(feature = "media")]
                    if let Some(sel) = op.select {
                        match sel {
                            DecodeSelection::All => {}
                            DecodeSelection::PresentationIndex { track, index } => {
                                if track.is_some() {
                                    return Err(at!(PipeError::Op(
                                        "track selection is not valid on animated image inputs"
                                            .to_string(),
                                    )));
                                }
                                let idx = usize::try_from(index).map_err(|_| {
                                    at!(PipeError::Op(
                                        "presentation_index out of range".to_string()
                                    ))
                                })?;
                                let (px, _) = frames.into_iter().nth(idx).ok_or_else(|| {
                                    at!(PipeError::Op(
                                        "presentation_index exceeds animated image frame count"
                                            .to_string(),
                                    ))
                                })?;
                                return Ok(NodeValue::Still {
                                    pixels: px,
                                    metadata,
                                    source_format,
                                    raw_bytes: None,
                                });
                            }
                            DecodeSelection::PresentationTime { track, .. } => {
                                if track.is_some() {
                                    return Err(at!(PipeError::Op(
                                        "track selection is not valid on animated image inputs"
                                            .to_string(),
                                    )));
                                }
                                let durs: Vec<FrameDuration> = frames
                                    .iter()
                                    .map(|(_, d)| {
                                        d.unwrap_or_else(|| FrameDuration::from_millis(1))
                                    })
                                    .collect();
                                let idx = select_frame_by_time(&durs, sel)?;
                                let (px, _) = frames.into_iter().nth(idx).expect("valid idx");
                                return Ok(NodeValue::Still {
                                    pixels: px,
                                    metadata,
                                    source_format,
                                    raw_bytes: None,
                                });
                            }
                            DecodeSelection::Page(_) => {
                                return Err(at!(PipeError::Op(
                                    "page selection is not valid on animated image inputs"
                                        .to_string(),
                                )));
                            }
                            _ => {}
                        }
                    }
                    #[cfg(not(feature = "media"))]
                    if let Some(index) = op.select_frame {
                        let idx = usize::try_from(index).map_err(|_| {
                            at!(PipeError::Op("select_frame out of range".to_string()))
                        })?;
                        let (px, _) = frames.into_iter().nth(idx).ok_or_else(|| {
                            at!(PipeError::Op(
                                "select_frame exceeds animated image frame count".to_string()
                            ))
                        })?;
                        return Ok(NodeValue::Still {
                            pixels: px,
                            metadata,
                            source_format,
                            raw_bytes: None,
                        });
                    }
                    return Ok(NodeValue::Sequence {
                        frames,
                        loops,
                        metadata,
                        source_format,
                        #[cfg(feature = "media")]
                        source_tracks: None,
                        #[cfg(feature = "media")]
                        source_video_track_idx: None,
                    });
                }

                // Still image: validate selection before decoding.
                #[cfg(feature = "media")]
                if let Some(sel) = op.select {
                    match sel {
                        DecodeSelection::All => {}
                        DecodeSelection::PresentationIndex {
                            track: None,
                            index: 0,
                        } => {}
                        DecodeSelection::Page(0) => {}
                        _ => {
                            return Err(at!(PipeError::Op(
                                "decode.select requested non-zero or time-based presentation on still image input"
                                    .to_string(),
                            )));
                        }
                    }
                }
                #[cfg(not(feature = "media"))]
                if let Some(idx) = op.select_frame
                    && idx > 0
                {
                    return Err(at!(PipeError::Op(format!(
                        "select_frame {idx} exceeds still image input"
                    ))));
                }

                let decode_limits = mem.limits_with_remaining(limits);
                let decode_run = self
                    .services
                    .build_run_with_limits_and_pulse(decode_limits, run.pulse().clone());
                let probed_still = self
                    .engine
                    .probe(bytes, &decode_run)
                    .map_err(map_engine_err)?;
                let pw = probed_still.inspection().envelope.width;
                let ph = probed_still.inspection().envelope.height;
                if pw > 0 && ph > 0 {
                    let est_bytes = u64::from(pw)
                        .saturating_mul(u64::from(ph))
                        .saturating_mul(3);
                    mem.check_transient(est_bytes)?;
                }
                let needs = DecodeNeeds::still().with_orientation(op.orientation);
                let comp = self
                    .engine
                    .decode(&needs, bytes, &decode_run)
                    .map_err(map_engine_err)?;
                let decoded = comp.output;
                let source_format = Some(decoded.format());
                let metadata = decoded.metadata();
                let pixels = decoded.into_buffer();
                let decoded_bytes = check_canvas_limits(
                    pixels.width(),
                    pixels.height(),
                    pixels.format().layout.bytes_per_pixel(),
                    limits,
                )? as u64;
                mem.charge(decoded_bytes)?;
                Ok(NodeValue::Still {
                    pixels,
                    metadata,
                    source_format,
                    raw_bytes: Some(bytes),
                })
            }
        }
    }

    fn exec_encode(
        &self,
        src: &NodeValue<'io>,
        enc: &EncodeNodeOp,
        cs: &ColorServices,
        run: &Run<'_>,
        limits: &ResourceLimits,
        mem: &mut LiveMemBudget,
    ) -> PipeResult<OutputArtifact> {
        // Route `Coding::KeepBytes` through `Engine::transcode` so metadata, orientation,
        // routing, and output byte limit gates run identically instead of bypassing checks!
        if enc.intent.coding == Coding::KeepBytes {
            if let NodeValue::Still {
                pixels,
                raw_bytes: Some(raw),
                ..
            } = src
            {
                let comp = self
                    .engine
                    .transcode(&enc.intent, raw, run)
                    .map_err(map_engine_err)?;
                let out_bytes = comp.output.as_bytes().to_vec();
                if let Some(max_out) = limits.max_output_bytes
                    && (out_bytes.len() as u64) > max_out
                {
                    return Err(at!(PipeError::LimitExceeded(format!(
                        "KeepBytes output {} B exceeds max_output_bytes {max_out}",
                        out_bytes.len()
                    ))));
                }
                let fmt = comp.report.output_format.unwrap_or(ImageFormat::Png);
                return Ok(OutputArtifact {
                    slot: enc.slot.clone(),
                    kind: "still",
                    mime: fmt.mime_type().to_string(),
                    bytes: out_bytes,
                    pixels: None,
                    width: pixels.width(),
                    height: pixels.height(),
                    frames: None,
                    video_codec: None,
                    report: comp.report,
                });
            }
            return Err(at!(PipeError::Op(
                "Coding::KeepBytes requires unmodified encoded source bytes".to_string()
            )));
        }

        match (&enc.intent.target, src) {
            #[cfg(feature = "media")]
            (TargetFormat::Video { container, codec }, NodeValue::VideoStream { bytes, .. }) => {
                let comp = transcode_media_bytes(&self.engine, bytes, &enc.intent, run)
                    .map_err(map_engine_err)?;
                let out_bytes = comp.output.as_bytes().to_vec();
                let (w, h, nframes) =
                    probe_video_dims_best_effort(&self.engine, &out_bytes, run, limits);
                Ok(OutputArtifact {
                    slot: enc.slot.clone(),
                    kind: "video",
                    mime: container_mime(container).to_string(),
                    bytes: out_bytes,
                    pixels: None,
                    width: w,
                    height: h,
                    frames: nframes,
                    video_codec: Some(video_codec_token(codec).to_string()),
                    report: comp.report,
                })
            }
            #[cfg(feature = "media")]
            (
                TargetFormat::Video { container, codec },
                NodeValue::Sequence {
                    frames,
                    loops,
                    source_tracks,
                    source_video_track_idx,
                    ..
                },
            ) => {
                if let Some(tracks) = source_tracks {
                    let wanted = validate_track_policy(tracks, &enc.intent.tracks, true)
                        .map_err(map_engine_err)?;
                    if let Some(materialized) = source_video_track_idx
                        && *materialized != wanted
                    {
                        return Err(at!(PipeError::Op(format!(
                            "TrackPolicy selected video track {wanted}, but upstream geometry/filter materialized track {materialized}"
                        ))));
                    }
                }
                let mut views: Vec<(PixelView<'_>, FrameDuration)> =
                    Vec::with_capacity(frames.len());
                for (b, d_opt) in frames {
                    let dur = d_opt.ok_or_else(|| {
                        at!(PipeError::Op(
                            "source video frame is missing explicit duration; refusing to fabricate timing"
                                .to_string(),
                        ))
                    })?;
                    views.push((b.as_view(), dur));
                }
                let (w, h) = (frames[0].0.width(), frames[0].0.height());
                let count = match enc.intent.sequence {
                    SequencePolicy::FirstPresentation => 1u64,
                    SequencePolicy::Preserve { .. } => frames.len() as u64,
                    _ => frames.len() as u64,
                };
                let comp = encode_frames_to_video(&self.engine, &views, *loops, &enc.intent, run)
                    .map_err(map_engine_err)?;
                Ok(OutputArtifact {
                    slot: enc.slot.clone(),
                    kind: "video",
                    mime: container_mime(container).to_string(),
                    bytes: comp.output.as_bytes().to_vec(),
                    pixels: None,
                    width: w,
                    height: h,
                    frames: Some(count),
                    video_codec: Some(video_codec_token(codec).to_string()),
                    report: comp.report,
                })
            }
            #[cfg(feature = "media")]
            (TargetFormat::Video { container, codec }, NodeValue::Still { pixels, .. }) => {
                let dur = FrameDuration::from_millis(40);
                let views = [(pixels.as_view(), dur)];
                let comp = encode_frames_to_video(
                    &self.engine,
                    &views,
                    LoopCount::once(),
                    &enc.intent,
                    run,
                )
                .map_err(map_engine_err)?;
                Ok(OutputArtifact {
                    slot: enc.slot.clone(),
                    kind: "video",
                    mime: container_mime(container).to_string(),
                    bytes: comp.output.as_bytes().to_vec(),
                    pixels: None,
                    width: pixels.width(),
                    height: pixels.height(),
                    frames: Some(1),
                    video_codec: Some(video_codec_token(codec).to_string()),
                    report: comp.report,
                })
            }
            #[cfg(feature = "media")]
            (_, NodeValue::VideoStream { bytes, .. }) => {
                // Video -> still (FirstPresentation) or video -> animation (WebP/GIF).
                match enc.intent.sequence {
                    SequencePolicy::FirstPresentation => {
                        let poster = extract_poster_from_bytes(
                            &self.engine,
                            bytes,
                            DecodeSelection::PresentationIndex {
                                track: None,
                                index: 0,
                            },
                            PixelFormat::RGB8_SRGB,
                            run,
                        )
                        .map_err(map_engine_err)?;
                        let buf = poster.output.0;
                        let comp = self
                            .engine
                            .encode_pixels(&enc.intent, buf.as_view(), &Metadata::none(), None, run)
                            .map_err(map_engine_err)?;
                        let fmt = comp.report.output_format.unwrap_or(ImageFormat::Png);
                        Ok(OutputArtifact {
                            slot: enc.slot.clone(),
                            kind: "still",
                            mime: fmt.mime_type().to_string(),
                            bytes: comp.output.as_bytes().to_vec(),
                            pixels: None,
                            width: buf.width(),
                            height: buf.height(),
                            frames: Some(1),
                            video_codec: None,
                            report: comp.report,
                        })
                    }
                    SequencePolicy::Preserve { .. } | _ => {
                        let (frames, tracks, v_idx, report) = decode_all_video_frames(
                            &self.engine,
                            bytes,
                            Some(&enc.intent.tracks),
                            run,
                            limits,
                            mem,
                        )?;
                        let wanted = validate_track_policy(&tracks, &enc.intent.tracks, true)
                            .map_err(map_engine_err)?;
                        if wanted != v_idx {
                            return Err(at!(PipeError::Op(format!(
                                "selected video track {wanted} did not match decoded track {v_idx}"
                            ))));
                        }
                        self.encode_sequence_to_animation(
                            &frames,
                            LoopCount::once(),
                            &Metadata::none(),
                            enc,
                            cs,
                            run,
                            limits,
                            mem,
                            report,
                        )
                    }
                }
            }
            (
                _,
                NodeValue::Sequence {
                    frames,
                    loops,
                    metadata,
                    #[cfg(feature = "media")]
                    source_tracks,
                    #[cfg(feature = "media")]
                    source_video_track_idx,
                    ..
                },
            ) => {
                #[cfg(feature = "media")]
                if let Some(tracks) = source_tracks {
                    let wanted = validate_track_policy(tracks, &enc.intent.tracks, true)
                        .map_err(map_engine_err)?;
                    if let Some(materialized) = source_video_track_idx
                        && *materialized != wanted
                    {
                        return Err(at!(PipeError::Op(format!(
                            "TrackPolicy selected video track {wanted}, but upstream geometry/filter materialized track {materialized}"
                        ))));
                    }
                }
                match enc.intent.sequence {
                    SequencePolicy::FirstPresentation => {
                        let buf = &frames[0].0;
                        let still_run = self.services.build_run_with_limits_and_pulse(
                            mem.limits_with_remaining(limits),
                            run.pulse().clone(),
                        );
                        let comp = self
                            .engine
                            .encode_pixels(&enc.intent, buf.as_view(), metadata, None, &still_run)
                            .map_err(map_engine_err)?;
                        let fmt = comp.report.output_format.unwrap_or(ImageFormat::Png);
                        let out_bytes = comp.output.into_vec();
                        mem.charge(out_bytes.len() as u64)?;
                        Ok(OutputArtifact {
                            slot: enc.slot.clone(),
                            kind: "still",
                            mime: fmt.mime_type().to_string(),
                            bytes: out_bytes,
                            pixels: None,
                            width: buf.width(),
                            height: buf.height(),
                            frames: Some(1),
                            video_codec: None,
                            report: comp.report,
                        })
                    }
                    SequencePolicy::Preserve { .. } | _ => self.encode_sequence_to_animation(
                        frames,
                        *loops,
                        metadata,
                        enc,
                        cs,
                        run,
                        limits,
                        mem,
                        RouteReport::default(),
                    ),
                }
            }
            (
                _,
                NodeValue::Still {
                    pixels,
                    metadata,
                    source_format,
                    ..
                },
            ) => {
                let still_run = self.services.build_run_with_limits_and_pulse(
                    mem.limits_with_remaining(limits),
                    run.pulse().clone(),
                );
                let comp = self
                    .engine
                    .encode_pixels(
                        &enc.intent,
                        pixels.as_view(),
                        metadata,
                        *source_format,
                        &still_run,
                    )
                    .map_err(map_engine_err)?;
                let fmt = comp
                    .report
                    .output_format
                    .or(*source_format)
                    .unwrap_or(ImageFormat::Png);
                let out_bytes = comp.output.into_vec();
                mem.charge(out_bytes.len() as u64)?;
                Ok(OutputArtifact {
                    slot: enc.slot.clone(),
                    kind: "still",
                    mime: fmt.mime_type().to_string(),
                    bytes: out_bytes,
                    pixels: None,
                    width: pixels.width(),
                    height: pixels.height(),
                    frames: None,
                    video_codec: None,
                    report: comp.report,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_sequence_to_animation(
        &self,
        frames: &[(PixelBuf, Option<FrameDuration>)],
        source_loops: LoopCount,
        metadata: &Metadata,
        enc: &EncodeNodeOp,
        cs: &ColorServices,
        run: &Run<'_>,
        limits: &ResourceLimits,
        mem: &mut LiveMemBudget,
        mut base_report: RouteReport,
    ) -> PipeResult<OutputArtifact> {
        if frames.is_empty() {
            return Err(at!(PipeError::Op(
                "cannot encode empty frame sequence".to_string()
            )));
        }
        let (timing_policy, loop_policy) = match enc.intent.sequence {
            SequencePolicy::Preserve { timing, loops } => (timing, loops),
            SequencePolicy::FirstPresentation => {
                (TimingPolicy::Exact, LoopPolicy::Set(LoopCount::once()))
            }
            _ => (TimingPolicy::Exact, LoopPolicy::Preserve),
        };
        let effective_loops = match loop_policy {
            LoopPolicy::Preserve => source_loops,
            LoopPolicy::Set(explicit) => explicit,
            _ => source_loops,
        };

        let mut durs: Vec<FrameDuration> = Vec::with_capacity(frames.len());
        for (_, d_opt) in frames {
            let d = d_opt.ok_or_else(|| {
                at!(PipeError::Op(
                    "source frame is missing explicit duration; refusing to fabricate timing"
                        .to_string(),
                ))
            })?;
            durs.push(d);
        }

        // Enforce TimingPolicy on millisecond animation container clock (WebP/GIF) BEFORE encoding,
        // identically whether or not the `media` feature is compiled in!
        let ms_clock = TimeBase::new(1, 1000).expect("valid 1/1000 clock");
        #[cfg(feature = "media")]
        let ms_ticks: Vec<u64> = quantize_endpoints(&durs, ms_clock, timing_policy)
            .map_err(map_engine_err)?
            .durations;
        #[cfg(not(feature = "media"))]
        let ms_ticks: Vec<u64> = quantize_endpoints_exact_ms(&durs, ms_clock, timing_policy)?;

        let (w, h) = (frames[0].0.width(), frames[0].0.height());
        let env = zencodecs::engine::MediaEnvelope {
            kind: MediaKind::Animation,
            image_format: None,
            #[cfg(feature = "media")]
            container: None,
            width: w,
            height: h,
            has_alpha: true,
            frame_count: u32::try_from(frames.len()).ok(),
            orientation: Orientation::Identity,
            has_gain_map: false,
            has_non_video_tracks: false,
        };
        let mut eff = self.engine.resolve_run(run).map_err(map_engine_err)?;
        let offers = self
            .engine
            .encode_offers(&enc.intent, &env, run)
            .map_err(map_engine_err)?;
        let offer = offers.first().ok_or_else(|| {
            at!(PipeError::Op(
                "no registered animation encoder for target format".to_string()
            ))
        })?;
        // Admit workspace under Strict/BestEffort + PolicySet just like still & video paths!
        eff.admit_workspace(offer.id().as_str(), offer.workspace(), &mut base_report)
            .map_err(map_engine_err)?;
        let target_fmt = offer.format();
        let entry = self
            .engine
            .codecs()
            .factories::<ImageEncodeRole>(&target_fmt)
            .find(|e| e.id == *offer.id())
            .ok_or_else(|| {
                at!(PipeError::Op(format!(
                    "selected animation encoder '{}' not found in CodecSet",
                    offer.id().as_str()
                )))
            })?;

        let fidelity = match &enc.intent.coding {
            Coding::Auto => None,
            Coding::CalibratedQuality(q) => Some(Fidelity::codec_quality(*q)),
            Coding::QualityProfile { level, dpr } => {
                Some(Fidelity::codec_quality(level.calibrated_quality(*dpr)))
            }
            Coding::AimFor(t) => {
                t.validate().map_err(map_engine_err)?;
                if let Some(ref rev) = t.revision
                    && rev != zencodecs::engine::AIM_SEED_REVISION
                {
                    return Err(map_engine_err(at!(EngineError::simple(
                        zencodecs::engine::Stage::Admit,
                        zencodecs::engine::Failure::UnsupportedEffect,
                        "Coding::AimFor calibration revision does not match pinned target revision",
                    ))));
                }
                let est_q = aim_seed(t);
                base_report.aim_metric = Some(t.metric);
                base_report.aim_target = Some(t.value);
                base_report.aim_revision = Some(zencodecs::engine::AIM_SEED_REVISION.to_string());
                base_report.aim_estimated_quality = Some(est_q);
                Some(Fidelity::codec_quality(est_q))
            }
            Coding::Lossless => Some(Fidelity::Lossless),
            Coding::LosslessOrKeep => Some(Fidelity::codec_quality(
                QualityLevel::Good.calibrated_quality(1.0),
            )),
            Coding::Verify(_) => {
                return Err(at!(PipeError::Op(
                    "Coding::Verify is qualified for still SDR images only, not animations"
                        .to_string(),
                )));
            }
            Coding::KeepBytes | Coding::ExactJpegRoundTrip | _ => {
                return Err(at!(PipeError::Op(
                    "still-only passthrough/JPEG coding mode is not supported on animations"
                        .to_string(),
                )));
            }
        };
        let mut job = match fidelity {
            Some(f) => entry.factory.config().try_fidelity_job(f).map_err(|e| {
                at!(PipeError::Op(format!(
                    "animation encoder refused requested fidelity: {e}"
                )))
            })?,
            None => entry.factory.config().dyn_job(),
        };
        job.set_limits(*eff.resources.limits());
        job.set_pulse(eff.resources.pulse().clone());
        job.set_canvas_size(w, h);
        let loop_u32 = match effective_loops {
            LoopCount::Infinite => Some(0u32),
            LoopCount::Plays(n) => Some(n.get()),
            LoopCount::Unspecified | _ => None,
        };
        job.set_loop_count(loop_u32);
        let filtered_meta = metadata.filtered(&enc.intent.metadata.to_zencodec());
        job.set_unfiltered_metadata(filtered_meta);

        let mut anim_enc = job.into_animation_frame_encoder().map_err(|e| {
            at!(PipeError::Op(format!(
                "provider '{}' does not support animated encoding for {target_fmt:?}: {e}",
                offer.id().as_str()
            )))
        })?;

        for ((buf, _), &dur_ms) in frames.iter().zip(ms_ticks.iter()) {
            eff.check_stop(zencodecs::engine::Stage::Execute)
                .map_err(map_engine_err)?;
            let dur = FrameDuration::new(dur_ms, 1000).map_err(|_| {
                at!(PipeError::Op(
                    "quantized animation frame duration invalid".to_string()
                ))
            })?;
            if *buf.format() == PixelFormat::RGBA8_SRGB {
                let _ = anim_enc
                    .push_frame_timed(buf.as_view(), dur, Some(eff.resources.pulse()))
                    .map_err(|e| at!(PipeError::Op(format!("animation push_frame failed: {e}"))))?;
            } else {
                let rgba = convert_pixel_buf(
                    buf,
                    &PixelFormat::RGBA8_SRGB,
                    cs,
                    false,
                    limits,
                    mem,
                    run.pulse(),
                )?;
                let rgba_sz =
                    (rgba.as_view().row_bytes() as u64).saturating_mul(u64::from(rgba.height()));
                let push_res = anim_enc
                    .push_frame_timed(rgba.as_view(), dur, Some(eff.resources.pulse()))
                    .map_err(|e| at!(PipeError::Op(format!("animation push_frame failed: {e}"))));
                drop(rgba);
                mem.release(rgba_sz);
                let _ = push_res?;
            }
        }
        let encoded = anim_enc
            .finish(Some(eff.resources.pulse()))
            .map_err(|e| at!(PipeError::Op(format!("animation finish failed: {e}"))))?;
        let out_bytes = encoded.into_vec();
        if let Some(max_out) = eff.resources.limits().max_output_bytes
            && (out_bytes.len() as u64) > max_out
        {
            return Err(at!(PipeError::LimitExceeded(format!(
                "encoded animation {} B exceeds max_output_bytes {max_out}",
                out_bytes.len()
            ))));
        }
        base_report.encoder = Some(offer.id().clone());
        base_report.output_format = Some(target_fmt);
        Ok(OutputArtifact {
            slot: enc.slot.clone(),
            kind: "animation",
            mime: target_fmt.mime_type().to_string(),
            bytes: out_bytes,
            pixels: None,
            width: w,
            height: h,
            frames: Some(frames.len() as u64),
            video_codec: None,
            report: base_report,
        })
    }
}

fn aim_seed(target: &MetricTarget) -> f32 {
    match target.metric {
        Metric::Ssimulacra2 | Metric::ZensimA | Metric::ZensimB => target.value.clamp(10.0, 99.0),
        Metric::ButteraugliMax => (100.0 - target.value * 12.0).clamp(15.0, 98.0),
        _ => target.value.clamp(10.0, 99.0),
    }
}

fn map_engine_err(err: whereat::At<EngineError>) -> whereat::At<PipeError> {
    let (engine_err, _trace) = err.decompose();
    match engine_err.failure() {
        zencodecs::engine::Failure::Stopped(reason) => at!(PipeError::Stopped(*reason)),
        zencodecs::engine::Failure::ResourceLimit => {
            at!(PipeError::LimitExceeded(engine_err.to_string()))
        }
        _ => at!(PipeError::Codec(alloc::boxed::Box::new(engine_err))),
    }
}

#[cfg(feature = "media")]
fn select_frame_by_time(durs: &[FrameDuration], selection: DecodeSelection) -> PipeResult<usize> {
    let mut lcm_den: u128 = 1;
    for d in durs {
        let num = u128::from(d.numerator());
        let den = u128::from(d.denominator());
        let g = gcd_u128(num, den);
        let red_den = den / g;
        let lg = gcd_u128(lcm_den, red_den);
        lcm_den = (lcm_den / lg).checked_mul(red_den).ok_or_else(|| {
            at!(PipeError::Op(
                "frame timeline denominator overflow".to_string()
            ))
        })?;
        if lcm_den > u128::from(u32::MAX) {
            return Err(at!(PipeError::Op(
                "frame timeline common denominator exceeds TimeBase range".to_string()
            )));
        }
    }
    let tb = TimeBase::new(1, (lcm_den as u32).max(1))
        .map_err(|_| at!(PipeError::Op("invalid TimeBase".to_string())))?;
    let mut entries = Vec::with_capacity(durs.len());
    let mut acc_ticks: i64 = 0;
    for (i, d) in durs.iter().enumerate() {
        entries.push(PresentationEntry {
            presentation_index: i as u64,
            pts: Timestamp::new(acc_ticks, tb),
            decoded_keyframe: true,
            config_epoch: 0,
        });
        let step = u128::from(d.numerator()) * (lcm_den / u128::from(d.denominator()));
        acc_ticks = acc_ticks.saturating_add(i64::try_from(step).unwrap_or(i64::MAX));
    }
    let idx = build_presentation_index(entries, durs.len().max(1)).map_err(map_engine_err)?;
    select_presentation_index(&idx, selection).map_err(map_engine_err)
}

#[cfg(feature = "media")]
fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

#[cfg(not(feature = "media"))]
fn mul_u128_wide(a: u128, b: u128) -> (u128, u128) {
    let a_lo = (a as u64) as u128;
    let a_hi = a >> 64;
    let b_lo = (b as u64) as u128;
    let b_hi = b >> 64;
    let lo_lo = a_lo * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_lo = a_hi * b_lo;
    let hi_hi = a_hi * b_hi;
    let (mid, c1) = (lo_lo >> 64).overflowing_add(lo_hi);
    let (mid, c2) = mid.overflowing_add(hi_lo);
    let lo = (lo_lo & ((1u128 << 64) - 1)) | (mid << 64);
    let hi = hi_hi + (mid >> 64) + ((u128::from(c1) + u128::from(c2)) << 64);
    (hi, lo)
}

#[cfg(not(feature = "media"))]
fn quantize_endpoints_exact_ms(
    durations: &[FrameDuration],
    target_base: TimeBase,
    policy: TimingPolicy,
) -> PipeResult<Vec<u64>> {
    fn gcd(mut a: u128, mut b: u128) -> u128 {
        while b != 0 {
            let r = a % b;
            a = b;
            b = r;
        }
        a.max(1)
    }
    let tb_num = u128::from(target_base.numerator());
    let tb_den = u128::from(target_base.denominator());
    let mut out = Vec::with_capacity(durations.len());
    let mut acc_num: u128 = 0;
    let mut acc_den: u128 = 1;
    let mut prev_ticks: u128 = 0;
    for &dur in durations {
        let d_num = u128::from(dur.numerator());
        let d_den = u128::from(dur.denominator());
        let g = gcd(acc_den, d_den);
        let lcm = (acc_den / g)
            .checked_mul(d_den)
            .ok_or_else(|| at!(PipeError::Op("timeline denominator overflow".to_string())))?;
        let new_num = acc_num
            .checked_mul(d_den / g)
            .and_then(|a| {
                d_num
                    .checked_mul(acc_den / g)
                    .and_then(|b| a.checked_add(b))
            })
            .ok_or_else(|| at!(PipeError::Op("timeline numerator overflow".to_string())))?;
        let rg = gcd(new_num, lcm);
        acc_num = new_num / rg;
        acc_den = lcm / rg;

        let num_ticks = acc_num
            .checked_mul(tb_den)
            .ok_or_else(|| at!(PipeError::Op("tick numerator overflow".to_string())))?;
        let den_ticks = acc_den
            .checked_mul(tb_num)
            .ok_or_else(|| at!(PipeError::Op("tick denominator overflow".to_string())))?;
        let rem = num_ticks % den_ticks;
        let q_floor = num_ticks / den_ticks;

        let target_ticks = match policy {
            TimingPolicy::Exact => {
                if rem != 0 {
                    return Err(at!(PipeError::Op(
                        "frame duration is not exactly representable on millisecond animation clock under TimingPolicy::Exact"
                            .to_string(),
                    )));
                }
                q_floor
            }
            TimingPolicy::QuantizeEndpoints { max_boundary_error } => {
                let q_ticks = if rem
                    .checked_mul(2)
                    .is_none_or(|two_rem| two_rem >= den_ticks)
                {
                    q_floor
                        .checked_add(1)
                        .ok_or_else(|| at!(PipeError::Op("quantized tick overflow".to_string())))?
                } else {
                    q_floor
                };
                let lhs = q_ticks
                    .checked_mul(tb_num)
                    .and_then(|v| v.checked_mul(acc_den))
                    .ok_or_else(|| {
                        at!(PipeError::Op("quantization error lhs overflow".to_string()))
                    })?;
                let rhs = acc_num.checked_mul(tb_den).ok_or_else(|| {
                    at!(PipeError::Op("quantization error rhs overflow".to_string()))
                })?;
                let err_num = lhs.abs_diff(rhs);
                let err_den = tb_den.checked_mul(acc_den).ok_or_else(|| {
                    at!(PipeError::Op(
                        "quantization error denom overflow".to_string()
                    ))
                })?;
                let m_num = u128::from(max_boundary_error.numerator());
                let m_den = u128::from(max_boundary_error.denominator());
                if mul_u128_wide(err_num, m_den) > mul_u128_wide(m_num, err_den) {
                    return Err(at!(PipeError::Op(
                        "cumulative quantized frame boundary error exceeded max_boundary_error"
                            .to_string(),
                    )));
                }
                q_ticks
            }
            _ => {
                return Err(at!(PipeError::Op(
                    "unsupported TimingPolicy variant".to_string()
                )));
            }
        };
        let delta = u64::try_from(target_ticks.saturating_sub(prev_ticks))
            .map_err(|_| at!(PipeError::Op("quantized duration overflow".to_string())))?;
        out.push(delta);
        prev_ticks = target_ticks;
    }
    Ok(out)
}

/// Structural, zero-pixel-allocation GIF87a/GIF89a frame counter (O(1) RAM).
/// Returns `Some(count)` only when the byte stream parses cleanly through GIF block headers;
/// returns `None` on malformed/truncated structure.
fn count_gif_frames(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 13 || (&bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a") {
        return None;
    }
    let packed = bytes[10];
    let mut pos: usize = 13;
    if (packed & 0x80) != 0 {
        let gct_entries = 1usize << ((packed & 0x07) + 1);
        pos = pos.checked_add(gct_entries * 3)?;
        if pos > bytes.len() {
            return None;
        }
    }
    let mut frames: usize = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            0x3B => return Some(frames),
            0x2C => {
                pos = pos.checked_add(10)?;
                if pos > bytes.len() {
                    return None;
                }
                let img_packed = bytes[pos - 1];
                if (img_packed & 0x80) != 0 {
                    let lct_entries = 1usize << ((img_packed & 0x07) + 1);
                    pos = pos.checked_add(lct_entries * 3)?;
                    if pos > bytes.len() {
                        return None;
                    }
                }
                pos = pos.checked_add(1)?; // LZW minimum code size
                if pos > bytes.len() {
                    return None;
                }
                loop {
                    let sub_len = *bytes.get(pos)? as usize;
                    pos = pos.checked_add(1)?;
                    if sub_len == 0 {
                        break;
                    }
                    pos = pos.checked_add(sub_len)?;
                    if pos > bytes.len() {
                        return None;
                    }
                }
                frames = frames.checked_add(1)?;
            }
            0x21 => {
                pos = pos.checked_add(2)?;
                if pos > bytes.len() {
                    return None;
                }
                loop {
                    let sub_len = *bytes.get(pos)? as usize;
                    pos = pos.checked_add(1)?;
                    if sub_len == 0 {
                        break;
                    }
                    pos = pos.checked_add(sub_len)?;
                    if pos > bytes.len() {
                        return None;
                    }
                }
            }
            0x00 => pos += 1,
            _ => return None,
        }
    }
    Some(frames)
}

#[allow(clippy::type_complexity)]
fn decode_encoded_animation(
    engine: &Engine,
    bytes: &[u8],
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<(
    Vec<(PixelBuf, Option<FrameDuration>)>,
    LoopCount,
    Metadata,
    Option<ImageFormat>,
)> {
    let probed = engine.probe(bytes, run).map_err(map_engine_err)?;
    let fmt = probed.inspection().envelope.image_format.ok_or_else(|| {
        at!(PipeError::Op(
            "animated input missing image format".to_string()
        ))
    })?;
    let mut eff = engine.resolve_run(run).map_err(map_engine_err)?;
    let offers = engine
        .decode_offers(&probed, &DecodeNeeds::primary(), run)
        .map_err(map_engine_err)?;
    let offer = offers.first().ok_or_else(|| {
        at!(PipeError::Op(
            "no eligible registered decoder for animated image".to_string()
        ))
    })?;
    let mut report = RouteReport::default();
    eff.admit_workspace(offer.id().as_str(), offer.workspace(), &mut report)
        .map_err(map_engine_err)?;
    let ws_bytes = match offer.workspace() {
        zencodec::source::NativeBound::Exact(b) | zencodec::source::NativeBound::UpperBound(b) => b,
        _ => 0,
    };
    // For GIF decoders whose offer reports NativeBound::Unknown, reserve BOTH the owned
    // encoded input copy (`zengif` calls `data.into_owned()` on open!) AND the 9 B/px live
    // internal working buffers (1 B/px indexed + 4 B/px canvas + 4 B/px Restore-Previous
    // snapshot) BEFORE opening the decoder! Also validate GIF frame count structurally upfront.
    let pre_gif_frames = (fmt == ImageFormat::Gif)
        .then(|| count_gif_frames(bytes))
        .flatten();
    if fmt == ImageFormat::Gif && pre_gif_frames.is_none() && mem.limit.is_some() {
        return Err(at!(PipeError::LimitExceeded(
            "GIF animation frame count could not be structurally determined under finite max_memory_bytes"
                .to_string(),
        )));
    }
    let probed_px = u64::from(probed.inspection().envelope.width)
        .saturating_mul(u64::from(probed.inspection().envelope.height));
    let gif_metered_ws = if ws_bytes == 0 && fmt == ImageFormat::Gif {
        probed_px.saturating_mul(9)
    } else {
        0
    };
    let gif_input_copy = if fmt == ImageFormat::Gif {
        bytes.len() as u64
    } else {
        0
    };
    let mut extra_dec_ws = gif_metered_ws.saturating_add(gif_input_copy);
    let initial_ws = ws_bytes.saturating_add(extra_dec_ws);
    let provider_metered_ws = ws_bytes.saturating_add(gif_metered_ws);
    if initial_ws > 0 {
        mem.charge(initial_ws)?;
    }
    let entry = match engine
        .codecs()
        .factories::<zencodec::ImageDecodeRole>(&fmt)
        .find(|e| e.id == *offer.id())
    {
        Some(e) => e,
        None => {
            if initial_ws > 0 {
                mem.release(initial_ws);
            }
            return Err(at!(PipeError::Op(
                "selected animation decoder not found in CodecSet".to_string()
            )));
        }
    };
    let mut job = entry.factory.dyn_job();
    let mut dec_limits = mem.limits_with_remaining(eff.resources.limits());
    if mem.limit.is_some() && provider_metered_ws > 0 {
        // The decoder's own metered workspace (`provider_metered_ws`) is already reserved in `mem`
        // via `initial_ws`. Passing `mem.remaining()` would starve the decoder of its own reserved
        // workspace, while adding `mem.remaining()` would let the decoder consume credit reserved
        // for retained output frames. Cap the decoder to `provider_metered_ws` so it can use 100%
        // of its reserved workspace while keeping retained-frame credit strictly in `mem`.
        dec_limits.max_memory_bytes = Some(provider_metered_ws);
    }
    job.set_limits(dec_limits);
    job.set_pulse(eff.resources.pulse().clone());
    let mut dec = match job.into_animation_frame_decoder(Cow::Borrowed(bytes), &[]) {
        Ok(d) => d,
        Err(e) => {
            if initial_ws > 0 {
                mem.release(initial_ws);
            }
            return Err(at!(PipeError::Op(format!(
                "animation frame decoder open failed: {e}"
            ))));
        }
    };
    let loops = match dec.loop_count() {
        Some(0) => LoopCount::Infinite,
        Some(n) => NonZeroU32::new(n).map_or(LoopCount::Unspecified, LoopCount::Plays),
        None => LoopCount::Unspecified,
    };
    let metadata = Metadata::from(dec.info());
    let known_frame_count = dec.frame_count().map(|n| n as usize).or(pre_gif_frames);
    let canvas_px = u64::from(dec.info().width).saturating_mul(u64::from(dec.info().height));
    if ws_bytes == 0 && fmt == ImageFormat::Gif {
        let actual_gif_ws = canvas_px
            .saturating_mul(9)
            .saturating_add(bytes.len() as u64);
        if actual_gif_ws > extra_dec_ws {
            let delta = actual_gif_ws - extra_dec_ws;
            if let Err(e) = mem.charge(delta) {
                drop(dec);
                mem.release(initial_ws);
                return Err(e);
            }
            extra_dec_ws = actual_gif_ws;
        }
    }
    let total_dec_ws = ws_bytes.saturating_add(extra_dec_ws);
    if known_frame_count.is_none() && mem.limit.is_some() {
        drop(dec);
        if total_dec_ws > 0 {
            mem.release(total_dec_ws);
        }
        return Err(at!(PipeError::LimitExceeded(
            "animated decoder cannot establish total frame count for EOF detection under finite max_memory_bytes"
                .to_string(),
        )));
    }
    let est_frame_bytes = canvas_px.saturating_mul(4);
    let max_frames = limits.max_frames.unwrap_or(1_000_000) as usize;
    let mut frames = Vec::new();
    let decode_res: PipeResult<()> = (|| {
        loop {
            if let Some(fc) = known_frame_count
                && frames.len() >= fc
            {
                break;
            }
            mem.check_transient(est_frame_bytes)?;
            let Some(frame) = dec
                .render_next_frame_owned(Some(eff.resources.pulse()))
                .map_err(|e| at!(PipeError::Op(format!("animation frame decode failed: {e}"))))?
            else {
                break;
            };
            if frames.len() >= max_frames {
                return Err(at!(PipeError::LimitExceeded(format!(
                    "animated image frame count exceeds max_frames {max_frames}"
                ))));
            }
            let dur = frame.duration();
            let buf = frame.into_buffer();
            let frame_bytes = check_canvas_limits(
                buf.width(),
                buf.height(),
                buf.format().layout.bytes_per_pixel(),
                limits,
            )? as u64;
            mem.charge(frame_bytes)?;
            frames.push((buf, Some(dur)));
        }
        Ok(())
    })();
    drop(dec);
    if total_dec_ws > 0 {
        mem.release(total_dec_ws);
    }
    decode_res?;
    if frames.is_empty() {
        return Err(at!(PipeError::Op(
            "animated image decoded zero frames".to_string()
        )));
    }
    Ok((frames, loops, metadata, Some(fmt)))
}

fn clone_pixel_view(
    view: PixelView<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<PixelBuf> {
    let w = view.width();
    let h = view.height();
    let fmt = view.format().clone();
    let bpp = fmt.layout.bytes_per_pixel();
    let total = check_canvas_limits(w, h, bpp, limits)?;
    mem.charge(total as u64)?;
    let row_bytes = (w as usize) * bpp;
    let mut raw = Vec::new();
    raw.try_reserve_exact(total).map_err(|_| {
        at!(PipeError::LimitExceeded(
            "pixel clone allocation failed".to_string()
        ))
    })?;
    for y in 0..h {
        raw.extend_from_slice(&view.row(y)[..row_bytes]);
    }
    PixelBuf::from_bytes(raw, w, h, fmt).map_err(|e| at!(PipeError::Op(e.to_string())))
}

fn clone_node_value<'io>(
    val: &NodeValue<'io>,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    match val {
        NodeValue::Still {
            pixels,
            metadata,
            source_format,
            raw_bytes,
        } => {
            let sz =
                (pixels.as_view().row_bytes() as u64).saturating_mul(u64::from(pixels.height()));
            mem.charge(sz)?;
            Ok(NodeValue::Still {
                pixels: pixels.clone(),
                metadata: metadata.clone(),
                source_format: *source_format,
                raw_bytes: *raw_bytes,
            })
        }
        NodeValue::Sequence {
            frames,
            loops,
            metadata,
            source_format,
            #[cfg(feature = "media")]
            source_tracks,
            #[cfg(feature = "media")]
            source_video_track_idx,
        } => {
            for (b, _) in frames {
                let sz = (b.as_view().row_bytes() as u64).saturating_mul(u64::from(b.height()));
                mem.charge(sz)?;
            }
            Ok(NodeValue::Sequence {
                frames: frames.clone(),
                loops: *loops,
                metadata: metadata.clone(),
                source_format: *source_format,
                #[cfg(feature = "media")]
                source_tracks: source_tracks.clone(),
                #[cfg(feature = "media")]
                source_video_track_idx: *source_video_track_idx,
            })
        }
        #[cfg(feature = "media")]
        NodeValue::VideoStream { bytes, container } => Ok(NodeValue::VideoStream {
            bytes,
            container: container.clone(),
        }),
    }
}

fn materialize_first_still(
    val: &NodeValue<'_>,
    _engine: &Engine,
    _run: &Run<'_>,
    mem: &mut LiveMemBudget,
) -> PipeResult<PixelBuf> {
    match val {
        NodeValue::Still { pixels, .. } => {
            let sz =
                (pixels.as_view().row_bytes() as u64).saturating_mul(u64::from(pixels.height()));
            mem.charge(sz)?;
            Ok(pixels.clone())
        }
        NodeValue::Sequence { frames, .. } => {
            let first = &frames[0].0;
            let sz = (first.as_view().row_bytes() as u64).saturating_mul(u64::from(first.height()));
            mem.charge(sz)?;
            Ok(first.clone())
        }
        #[cfg(feature = "media")]
        NodeValue::VideoStream { bytes, .. } => {
            let comp = extract_poster_from_bytes(
                _engine,
                bytes,
                DecodeSelection::PresentationIndex {
                    track: None,
                    index: 0,
                },
                PixelFormat::RGB8_SRGB,
                _run,
            )
            .map_err(map_engine_err)?;
            let buf = comp.output.0;
            let sz = (buf.as_view().row_bytes() as u64).saturating_mul(u64::from(buf.height()));
            mem.charge(sz)?;
            Ok(buf)
        }
    }
}

#[cfg(feature = "media")]
#[allow(clippy::type_complexity)]
fn decode_all_video_frames(
    engine: &Engine,
    bytes: &[u8],
    track_policy: Option<&zencodecs::engine::TrackPolicy>,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<(
    Vec<(PixelBuf, Option<FrameDuration>)>,
    Vec<TrackInfo>,
    usize,
    RouteReport,
)> {
    use zencodec::decode::Input;
    use zencodec::source::SourceAccess;
    use zencodec_media::container::DemuxEvent;
    use zencodec_media::registry::{DemuxNeeds, VideoDecodeNeeds};
    use zencodec_media::session::{CodecConfig, Receive, SendStatus};
    use zencodec_media::timeline::{TimingEvidence, TrimTreatment};
    use zencodec_media::track::TrackKind;
    use zencodecs::media::pump::render_frame_to_pixel_buf;
    use zencodecs::media::route::{select_demuxer, select_video_decoder};
    use zencodecs::media::timeline::{MappedPresentation, TimelineBridge};

    let mut eff = engine.resolve_run(run).map_err(map_engine_err)?;
    eff.check_stop(zencodecs::engine::Stage::Inspect)
        .map_err(map_engine_err)?;
    let mut report = RouteReport {
        route: Some(zencodecs::engine::RouteKind::Media),
        ..Default::default()
    };
    let container = zencodecs::media::probe::probe_container_bytes(
        engine.codecs(),
        bytes,
        &zencodec_media::registry::ProbeLimits::default(),
    )
    .map_err(map_engine_err)?
    .ok_or_else(|| at!(PipeError::Op("unrecognized video container".to_string())))?
    .0;

    let (demux_id, demux_factory) =
        select_demuxer(engine, &container, &mut eff, &mut report).map_err(map_engine_err)?;
    let total_len = bytes.len() as u64;
    let access = SourceAccess::borrowed(total_len);
    let offer = demux_factory
        .quote(&DemuxNeeds::default(), &access, &eff.resources)
        .map_err(|e| at!(PipeError::Op(format!("demux quote failed: {e}"))))?;
    eff.admit_workspace(demux_id.as_str(), offer.workspace, &mut report)
        .map_err(map_engine_err)?;
    let mut demux = demux_factory
        .open(&offer, &eff.resources)
        .map_err(|e| at!(PipeError::Op(format!("demux open failed: {e}"))))?;

    let mut offset = 0usize;
    let mut tracks = Vec::new();
    let mut packets = Vec::new();
    let max_frames = limits.max_frames.unwrap_or(1_000_000) as usize;
    loop {
        eff.check_stop(zencodecs::engine::Stage::Execute)
            .map_err(map_engine_err)?;
        let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
        let slice = if offset <= bytes.len() {
            &bytes[offset..]
        } else {
            &[]
        };
        let prog = demux
            .step(
                Input::new(offset as u64, slice).with_total_len(total_len),
                pulse,
            )
            .map_err(|e| at!(PipeError::Op(format!("demux step failed: {e}"))))?;
        if prog.consumed > slice.len() {
            return Err(at!(PipeError::Op(
                "demuxer consumed more bytes than offered".to_string()
            )));
        }
        offset = offset.saturating_add(prog.consumed);
        match prog.event {
            DemuxEvent::Need => {
                if let Some(req) = demux.requested().first().copied() {
                    let target = req.offset().map(|o| o as usize).unwrap_or(offset);
                    if target == offset && offset >= bytes.len() {
                        break;
                    }
                    offset = target.min(bytes.len());
                } else {
                    break;
                }
            }
            DemuxEvent::Headers => tracks = demux.tracks().to_vec(),
            DemuxEvent::Packet(p) => packets.push(p),
            DemuxEvent::Done => break,
            _ => {}
        }
    }

    let v_idx = if let Some(tp) = track_policy {
        validate_track_policy(&tracks, tp, true).map_err(map_engine_err)?
    } else {
        tracks
            .iter()
            .position(|t| t.kind() == TrackKind::Video)
            .ok_or_else(|| at!(PipeError::Op("container has no video track".to_string())))?
    };
    let track = &tracks[v_idx];
    let cfg = CodecConfig::from(track);
    let extent = track
        .video_info()
        .filter(|v| v.coded_width > 0 && v.coded_height > 0)
        .map(|v| (v.coded_width, v.coded_height))
        .unwrap_or((4_096, 4_096));
    let need = VideoDecodeNeeds::new(extent, 16);
    let (_did, dec_factory, dec_plan) =
        select_video_decoder(engine, &cfg, &need, &mut eff, &mut report).map_err(map_engine_err)?;
    let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
    let mut dec = dec_factory
        .open(&dec_plan, &eff.resources)
        .map_err(|e| at!(PipeError::Op(format!("video decoder open failed: {e}"))))?;
    dec.configure(&cfg, pulse).map_err(|e| {
        at!(PipeError::Op(format!(
            "video decoder configure failed: {e}"
        )))
    })?;

    let mut tl = TimelineBridge::prepare(
        track,
        TimingEvidence::new(
            TrimTreatment::ApplyInPresentation,
            TrimTreatment::ApplyInPresentation,
        ),
        &eff.resources,
    )
    .map_err(map_engine_err)?;

    let mut out_frames: Vec<(PixelBuf, Option<FrameDuration>)> = Vec::new();
    let mut collect_ready = |tl: &mut TimelineBridge, mem: &mut LiveMemBudget| -> PipeResult<()> {
        for item in tl.drain_ready() {
            match item {
                MappedPresentation::Frame(f) => {
                    if out_frames.len() >= max_frames {
                        return Err(at!(PipeError::LimitExceeded(format!(
                            "video stream frame count exceeds max_frames {max_frames}"
                        ))));
                    }
                    let dur = f.timing().duration_ticks.and_then(|ticks| {
                        let tb = f
                            .timing()
                            .pts
                            .map(Timestamp::base)
                            .unwrap_or_else(|| track.time_base());
                        FrameDuration::new(
                            ticks.saturating_mul(u64::from(tb.numerator())),
                            tb.denominator(),
                        )
                        .ok()
                    });
                    let buf = render_frame_to_pixel_buf(&f, PixelFormat::RGBA8_SRGB, &eff)
                        .map_err(map_engine_err)?;
                    let frame_bytes = check_canvas_limits(
                        buf.width(),
                        buf.height(),
                        buf.format().layout.bytes_per_pixel(),
                        limits,
                    )? as u64;
                    mem.charge(frame_bytes)?;
                    out_frames.push((buf, dur));
                }
                MappedPresentation::Gap { .. } => {
                    return Err(at!(PipeError::Op(
                        "explicit timeline edit gap cannot be silently dropped when materializing video frames"
                            .to_string(),
                    )));
                }
            }
        }
        Ok(())
    };

    for mut pkt in packets.into_iter().filter(|p| p.track() as usize == v_idx) {
        eff.check_stop(zencodecs::engine::Stage::Execute)
            .map_err(map_engine_err)?;
        let pkt_pts = pkt.pts();
        let pkt_discard = pkt.discard();
        loop {
            match dec.send(&mut pkt, pulse) {
                Ok(SendStatus::Accepted) => {
                    tl.record_accepted_packet(pkt_pts, pkt_discard)
                        .map_err(map_engine_err)?;
                    break;
                }
                Ok(SendStatus::ReceivePending) => {
                    while let Ok(Receive::Ready(f)) = dec.receive(pulse) {
                        tl.push_frame(f).map_err(map_engine_err)?;
                        collect_ready(&mut tl, mem)?;
                    }
                }
                Ok(_) => break,
                Err(e) => return Err(at!(PipeError::Op(format!("video decode send failed: {e}")))),
            }
        }
        loop {
            match dec.receive(pulse) {
                Ok(Receive::Ready(f)) => {
                    tl.push_frame(f).map_err(map_engine_err)?;
                    collect_ready(&mut tl, mem)?;
                }
                Ok(Receive::Progress { .. }) => {}
                Ok(Receive::NeedInput | Receive::EndOfStream) => break,
                Ok(_) => {}
                Err(e) => {
                    return Err(at!(PipeError::Op(format!(
                        "video decode receive failed: {e}"
                    ))));
                }
            }
        }
    }
    dec.end_input(pulse)
        .map_err(|e| at!(PipeError::Op(format!("video decode end_input failed: {e}"))))?;
    loop {
        match dec.receive(pulse) {
            Ok(Receive::Ready(f)) => {
                tl.push_frame(f).map_err(map_engine_err)?;
                collect_ready(&mut tl, mem)?;
            }
            Ok(Receive::Progress { .. }) => {}
            Ok(Receive::EndOfStream | Receive::NeedInput) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(at!(PipeError::Op(format!(
                    "video decode drain failed: {e}"
                ))));
            }
        }
    }
    tl.finish().map_err(map_engine_err)?;
    collect_ready(&mut tl, mem)?;
    if out_frames.is_empty() {
        return Err(at!(PipeError::Op(
            "video stream produced zero decoded frames".to_string()
        )));
    }
    Ok((out_frames, tracks, v_idx, report))
}

#[cfg(feature = "media")]
fn probe_video_dims_best_effort(
    engine: &Engine,
    bytes: &[u8],
    run: &Run<'_>,
    limits: &ResourceLimits,
) -> (u32, u32, Option<u64>) {
    let mut scratch_mem = LiveMemBudget::new(limits);
    if let Ok((frames, _, _, _)) =
        decode_all_video_frames(engine, bytes, None, run, limits, &mut scratch_mem)
        && let Some((first, _)) = frames.first()
    {
        return (first.width(), first.height(), Some(frames.len() as u64));
    }
    (0, 0, None)
}

#[cfg(feature = "media")]
fn container_mime(c: &ContainerId) -> &'static str {
    match c {
        ContainerId::Mp4 => "video/mp4",
        ContainerId::Webm => "video/webm",
        ContainerId::Ivf => "video/x-ivf",
        _ => "application/octet-stream",
    }
}

#[cfg(feature = "media")]
fn video_codec_token(c: &MediaCodecId) -> &'static str {
    match c {
        MediaCodecId::Av1 => "av1",
        MediaCodecId::Vp8 => "vp8",
        MediaCodecId::Vp9 => "vp9",
        _ => "unknown",
    }
}

fn map_frames<'io, F>(
    val: &NodeValue<'io>,
    _engine: &Engine,
    _run: &Run<'_>,
    _limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
    mut f: F,
) -> PipeResult<NodeValue<'io>>
where
    F: FnMut(&PixelBuf, &mut LiveMemBudget) -> PipeResult<PixelBuf>,
{
    match val {
        NodeValue::Still {
            pixels,
            metadata,
            source_format,
            ..
        } => Ok(NodeValue::Still {
            pixels: f(pixels, mem)?,
            metadata: metadata.clone(),
            source_format: *source_format,
            raw_bytes: None,
        }),
        NodeValue::Sequence {
            frames,
            loops,
            metadata,
            source_format,
            #[cfg(feature = "media")]
            source_tracks,
            #[cfg(feature = "media")]
            source_video_track_idx,
        } => {
            let mut out = Vec::with_capacity(frames.len());
            for (buf, dur) in frames {
                out.push((f(buf, mem)?, *dur));
            }
            Ok(NodeValue::Sequence {
                frames: out,
                loops: *loops,
                metadata: metadata.clone(),
                source_format: *source_format,
                #[cfg(feature = "media")]
                source_tracks: source_tracks.clone(),
                #[cfg(feature = "media")]
                source_video_track_idx: *source_video_track_idx,
            })
        }
        #[cfg(feature = "media")]
        NodeValue::VideoStream { bytes, .. } => {
            let (frames, tracks, v_idx, _) =
                decode_all_video_frames(_engine, bytes, None, _run, _limits, mem)?;
            let decoded_bytes: u64 = frames.iter().fold(0u64, |acc, (b, _)| {
                acc.saturating_add(
                    (b.as_view().row_bytes() as u64).saturating_mul(u64::from(b.height())),
                )
            });
            let mut out = Vec::with_capacity(frames.len());
            for (buf, dur) in &frames {
                out.push((f(buf, mem)?, *dur));
            }
            drop(frames);
            mem.release(decoded_bytes);
            Ok(NodeValue::Sequence {
                frames: out,
                loops: LoopCount::once(),
                metadata: Metadata::none(),
                source_format: None,
                source_tracks: Some(tracks),
                source_video_track_idx: Some(v_idx),
            })
        }
    }
}

fn convert_pixel_buf(
    src: &PixelBuf,
    dst_fmt: &PixelFormat,
    cs: &ColorServices,
    strict_icc: bool,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
    pulse: &SharedPulse,
) -> PipeResult<PixelBuf> {
    pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
    let w = src.width();
    let h = src.height();
    let bpp = dst_fmt.layout.bytes_per_pixel();
    let total = check_canvas_limits(w, h, bpp, limits)?;
    if src.format() == dst_fmt {
        mem.charge(total as u64)?;
        return Ok(src.clone());
    }
    let dst_stride = (w as usize) * bpp;
    let mut dst_raw = alloc_zeroed_checked(total, limits, mem)?;
    let base_policy = if strict_icc {
        ConvertPolicy::default()
    } else {
        ConvertPolicy::permissive()
    };
    let policy = base_policy.with_color_services(cs);
    let plan = RowPlan::build(src.format(), dst_fmt, &policy, None)
        .map_err(|e| at!(PipeError::Op(format!("color conversion plan failed: {e}"))))?;
    let src_view = src.as_view();
    for y in 0..h {
        if y & 63 == 0 {
            pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
        }
        let srow = src_view.row(y);
        let d0 = (y as usize) * dst_stride;
        plan.convert_row(srow, &mut dst_raw[d0..d0 + dst_stride], w)
            .map_err(|e| at!(PipeError::Op(format!("row color conversion failed: {e}"))))?;
    }
    pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
    PixelBuf::from_bytes(dst_raw, w, h, dst_fmt.clone())
        .map_err(|e| at!(PipeError::Op(e.to_string())))
}

#[allow(clippy::too_many_arguments)]
fn exec_color_ingest<'io>(
    src: &NodeValue<'io>,
    mode: ColorIngestMode,
    target: Option<&ColorSpace>,
    strict_invalid_icc: bool,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    // When `strict_invalid_icc` is true and embedded ICC bytes are present on the input,
    // validate ICC structural integrity rather than silently ignoring malformed profiles.
    if strict_invalid_icc
        && let NodeValue::Still { metadata, .. } | NodeValue::Sequence { metadata, .. } = src
        && let Some(icc) = metadata.icc_profile.as_deref().filter(|b| !b.is_empty())
        && let Err(e) = zenpixels::validate_icc_structure(icc)
    {
        return Err(at!(PipeError::Op(format!(
            "color_ingest(strict_invalid_icc=true) rejected invalid embedded ICC profile: {e}"
        ))));
    }

    match mode {
        ColorIngestMode::KeepTagged => clone_node_value(src, mem),
        ColorIngestMode::Assign => {
            let tgt = target.ok_or_else(|| {
                at!(PipeError::Op(
                    "color_ingest.assign requires target".to_string()
                ))
            })?;
            map_frames(src, engine, run, limits, mem, |buf, mem| {
                let new_fmt = PixelFormat::new(buf.format().layout, tgt.clone());
                let raw = clone_pixel_view(buf.as_view(), limits, mem)?
                    .into_vec()
                    .map_err(|e| at!(PipeError::Op(e.to_string())))?;
                PixelBuf::from_bytes(raw, buf.width(), buf.height(), new_fmt)
                    .map_err(|e| at!(PipeError::Op(e.to_string())))
            })
        }
        ColorIngestMode::Convert => {
            let tgt = target.ok_or_else(|| {
                at!(PipeError::Op(
                    "color_ingest.convert requires target".to_string()
                ))
            })?;
            map_frames(src, engine, run, limits, mem, |buf, mem| {
                let new_fmt = PixelFormat::new(buf.format().layout, tgt.clone());
                convert_pixel_buf(
                    buf,
                    &new_fmt,
                    cs,
                    strict_invalid_icc,
                    limits,
                    mem,
                    run.pulse(),
                )
            })
        }
    }
}

fn exec_color_space<'io>(
    src: &NodeValue<'io>,
    to: &PixelFormat,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    map_frames(src, engine, run, limits, mem, |buf, mem| {
        convert_pixel_buf(buf, to, cs, false, limits, mem, run.pulse())
    })
}

fn exec_tone_map<'io>(
    src: &NodeValue<'io>,
    render: &zencodecs::engine::ToneMap,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    // Refuse HDR tone-mapping honestly rather than silently clamping/converting to sRGB
    // without executing the requested BT.2408 / BT.2446 operator!
    map_frames(src, engine, run, limits, mem, |buf, mem| {
        if buf.format().color.is_hdr() {
            return Err(at!(PipeError::Op(format!(
                "HDR tone_map execution ({:?}) is not yet implemented in canonical runtime",
                render.method
            ))));
        }
        let target_cs = match render.target_primaries {
            Primaries::Bt709 => ColorSpace::SRGB,
            Primaries::DisplayP3 => ColorSpace::DISPLAY_P3,
            other => {
                return Err(at!(PipeError::Op(format!(
                    "tone_map target primaries {other:?} is not supported on SDR pass-through"
                ))));
            }
        };
        let target_fmt = PixelFormat::new(buf.format().layout, target_cs);
        convert_pixel_buf(buf, &target_fmt, cs, false, limits, mem, run.pulse())
    })
}

fn geometry_src(g: &GeometryOp) -> crate::canonical::ImageId {
    match g {
        GeometryOp::Orient { src, .. }
        | GeometryOp::Crop { src, .. }
        | GeometryOp::Resize { src, .. }
        | GeometryOp::Constrain { src, .. }
        | GeometryOp::ExtendCanvas { src, .. } => *src,
    }
}

fn filter_src(f: &FilterNodeOp) -> crate::canonical::ImageId {
    match f {
        FilterNodeOp::Exposure { src, .. }
        | FilterNodeOp::Contrast { src, .. }
        | FilterNodeOp::Saturation { src, .. }
        | FilterNodeOp::Vibrance { src, .. }
        | FilterNodeOp::Blur { src, .. }
        | FilterNodeOp::Sharpen { src, .. }
        | FilterNodeOp::Grayscale { src }
        | FilterNodeOp::Sepia { src, .. } => *src,
    }
}

fn map_resize_kernel(k: ResizeKernel) -> ResizeFilter {
    match k {
        ResizeKernel::Robidoux => ResizeFilter::Robidoux,
        ResizeKernel::Lanczos3 => ResizeFilter::Lanczos,
        ResizeKernel::CatmullRom => ResizeFilter::CatmullRom,
        ResizeKernel::Linear => ResizeFilter::Linear,
        ResizeKernel::Nearest => ResizeFilter::Box,
    }
}

fn canvas_color_to_rgba8(c: CanvasColor) -> [u8; 4] {
    match c {
        CanvasColor::Transparent => [0, 0, 0, 0],
        CanvasColor::Srgb { r, g, b, a } => [r, g, b, a],
        CanvasColor::Linear { r, g, b, a } => {
            let enc = |v: f32| -> u8 {
                let c = v.clamp(0.0, 1.0);
                let s = if c <= 0.003_130_8 {
                    c * 12.92
                } else {
                    1.055 * c.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            };
            [
                enc(r),
                enc(g),
                enc(b),
                (a.clamp(0.0, 1.0) * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
            ]
        }
        _ => [0, 0, 0, 0],
    }
}

#[allow(clippy::too_many_arguments)]
fn exec_geometry<'io>(
    src: &NodeValue<'io>,
    op: &GeometryOp,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    map_frames(src, engine, run, limits, mem, |buf, mem| match op {
        GeometryOp::Orient { orientation, .. } => {
            apply_orient(buf, *orientation, limits, mem, run.pulse())
        }
        GeometryOp::Crop { rect, .. } => apply_crop(buf, *rect, limits, mem),
        GeometryOp::Resize {
            width,
            height,
            kernel,
            linear_light,
            ..
        } => {
            let (sw, sh) = (buf.width(), buf.height());
            let (tw, th) = match (*width, *height) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => {
                    let h_u128 = ((2u128 * u128::from(sh) * u128::from(w) + u128::from(sw))
                        / (2u128 * u128::from(sw)))
                    .max(1);
                    let h = u32::try_from(h_u128).map_err(|_| {
                        at!(PipeError::LimitExceeded(
                            "computed resize height exceeded u32::MAX".to_string()
                        ))
                    })?;
                    (w, h)
                }
                (None, Some(h)) => {
                    let w_u128 = ((2u128 * u128::from(sw) * u128::from(h) + u128::from(sh))
                        / (2u128 * u128::from(sh)))
                    .max(1);
                    let w = u32::try_from(w_u128).map_err(|_| {
                        at!(PipeError::LimitExceeded(
                            "computed resize width exceeded u32::MAX".to_string()
                        ))
                    })?;
                    (w, h)
                }
                (None, None) => (sw, sh),
            };
            apply_resize(buf, tw, th, *kernel, *linear_light, limits, mem)
        }
        GeometryOp::Constrain {
            constraint,
            kernel,
            linear_light,
            ..
        } => {
            let layout = constraint
                .compute(buf.width(), buf.height())
                .map_err(|e| at!(PipeError::Op(format!("layout constraint failed: {e}"))))?;
            // Execute the COMPLETE planned layout: source_crop -> resize_to -> canvas + placement!
            let (cropped, cropped_owned) = if let Some(crop_rect) = layout.source_crop {
                (apply_crop(buf, crop_rect, limits, mem)?, true)
            } else {
                let sz = (buf.as_view().row_bytes() as u64).saturating_mul(u64::from(buf.height()));
                mem.charge(sz)?;
                (buf.clone(), true)
            };
            let resized = if layout.resize_to.width != cropped.width()
                || layout.resize_to.height != cropped.height()
            {
                let old_sz = (cropped.as_view().row_bytes() as u64)
                    .saturating_mul(u64::from(cropped.height()));
                let r = apply_resize(
                    &cropped,
                    layout.resize_to.width,
                    layout.resize_to.height,
                    *kernel,
                    *linear_light,
                    limits,
                    mem,
                )?;
                if cropped_owned {
                    drop(cropped);
                    mem.release(old_sz);
                }
                r
            } else {
                cropped
            };
            if layout.needs_padding() || layout.placement != (0, 0) {
                let old_sz = (resized.as_view().row_bytes() as u64)
                    .saturating_mul(u64::from(resized.height()));
                let fill = canvas_color_to_rgba8(layout.canvas_color);
                let extended = apply_extend_canvas(
                    &resized,
                    layout.canvas,
                    layout.placement.0,
                    layout.placement.1,
                    fill,
                    cs,
                    limits,
                    mem,
                    run.pulse(),
                )?;
                drop(resized);
                mem.release(old_sz);
                Ok(extended)
            } else {
                Ok(resized)
            }
        }
        GeometryOp::ExtendCanvas {
            canvas,
            offset_x,
            offset_y,
            fill_rgba,
            ..
        } => apply_extend_canvas(
            buf,
            *canvas,
            *offset_x,
            *offset_y,
            *fill_rgba,
            cs,
            limits,
            mem,
            run.pulse(),
        ),
    })
}

fn apply_orient(
    buf: &PixelBuf,
    orientation: Orientation,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
    pulse: &SharedPulse,
) -> PipeResult<PixelBuf> {
    pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
    let (sw, sh) = (buf.width(), buf.height());
    let bpp = buf.format().layout.bytes_per_pixel();
    if orientation == Orientation::Identity {
        let total = check_canvas_limits(sw, sh, bpp, limits)?;
        mem.charge(total as u64)?;
        return Ok(buf.clone());
    }
    let out_size = orientation.transform_dimensions(sw, sh);
    let total = check_canvas_limits(out_size.width, out_size.height, bpp, limits)?;
    let dst_stride = (out_size.width as usize) * bpp;
    let mut dst = alloc_zeroed_checked(total, limits, mem)?;
    let view = buf.as_view();
    for sy in 0..sh {
        if sy & 63 == 0 {
            pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
        }
        let srow = view.row(sy);
        for sx in 0..sw {
            let (dx, dy) = orientation.map_pixel_coordinates((sx, sy), (sw, sh));
            let s0 = (sx as usize) * bpp;
            let d0 = (dy as usize) * dst_stride + (dx as usize) * bpp;
            dst[d0..d0 + bpp].copy_from_slice(&srow[s0..s0 + bpp]);
        }
    }
    pulse.check().map_err(|r| at!(PipeError::Stopped(r)))?;
    PixelBuf::from_bytes(dst, out_size.width, out_size.height, buf.format().clone())
        .map_err(|e| at!(PipeError::Op(e.to_string())))
}

fn apply_crop(
    buf: &PixelBuf,
    rect: Rect,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<PixelBuf> {
    let x_end = rect
        .x
        .checked_add(rect.width)
        .ok_or_else(|| at!(PipeError::DimensionMismatch("crop x overflow".to_string())))?;
    let y_end = rect
        .y
        .checked_add(rect.height)
        .ok_or_else(|| at!(PipeError::DimensionMismatch("crop y overflow".to_string())))?;
    if x_end > buf.width() || y_end > buf.height() {
        return Err(at!(PipeError::DimensionMismatch(format!(
            "crop rect ({},{},{}x{}) exceeds source {}x{}",
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            buf.width(),
            buf.height()
        ))));
    }
    let bpp = buf.format().layout.bytes_per_pixel();
    let total = check_canvas_limits(rect.width, rect.height, bpp, limits)?;
    mem.charge(total as u64)?;
    let row_bytes = (rect.width as usize) * bpp;
    let mut dst = Vec::new();
    dst.try_reserve_exact(total).map_err(|_| {
        at!(PipeError::LimitExceeded(
            "crop buffer allocation failed".to_string()
        ))
    })?;
    let view = buf.as_view();
    let sx0 = (rect.x as usize) * bpp;
    for y in rect.y..y_end {
        let row = view.row(y);
        dst.extend_from_slice(&row[sx0..sx0 + row_bytes]);
    }
    PixelBuf::from_bytes(dst, rect.width, rect.height, buf.format().clone())
        .map_err(|e| at!(PipeError::Op(e.to_string())))
}

fn apply_resize(
    buf: &PixelBuf,
    tw: u32,
    th: u32,
    kernel: ResizeKernel,
    linear_light: bool,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<PixelBuf> {
    let bpp = buf.format().layout.bytes_per_pixel();
    let dst_total = check_canvas_limits(tw, th, bpp, limits)?;
    if tw == buf.width() && th == buf.height() {
        mem.charge(dst_total as u64)?;
        return Ok(buf.clone());
    }
    let src_total = (buf.width() as u64)
        .saturating_mul(u64::from(buf.height()))
        .saturating_mul(bpp as u64);
    // Reserve budget for Resizer::resize's transient workspace + destination buffer before invoking it.
    let transient_est = (dst_total as u64).saturating_add(src_total);
    mem.check_transient(transient_est)?;
    let mut cfg = ResizeConfig::builder(buf.width(), buf.height(), tw, th)
        .filter(map_resize_kernel(kernel))
        .format(buf.format().clone())
        .build();
    cfg.linear = linear_light;
    let raw = buf.as_view().to_contiguous_bytes().map_err(|e| {
        at!(PipeError::Op(format!(
            "resize input contiguous failed: {e}"
        )))
    })?;
    let out = Resizer::new(&cfg).resize(&raw);
    mem.charge(out.len() as u64)?;
    PixelBuf::from_bytes(out, tw, th, buf.format().clone())
        .map_err(|e| at!(PipeError::Op(e.to_string())))
}

fn make_solid_canvas(
    size: Size,
    fill_rgba: [u8; 4],
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<PixelBuf> {
    let total = check_canvas_limits(size.width, size.height, 4, limits)?;
    mem.charge(total as u64)?;
    let px = (size.width as usize) * (size.height as usize);
    let mut raw = Vec::new();
    raw.try_reserve_exact(total).map_err(|_| {
        at!(PipeError::LimitExceeded(
            "canvas allocation failed".to_string()
        ))
    })?;
    for _ in 0..px {
        raw.extend_from_slice(&fill_rgba);
    }
    PixelBuf::from_bytes(raw, size.width, size.height, PixelFormat::RGBA8_SRGB)
        .map_err(|e| at!(PipeError::Op(e.to_string())))
}

fn alpha_capable_layout(layout: SampleLayout) -> PipeResult<SampleLayout> {
    use zenpixels::{BitPacking, ChannelOrder};
    let order = match layout.order {
        ChannelOrder::Rgb | ChannelOrder::Rgbx | ChannelOrder::Rgba => ChannelOrder::Rgba,
        ChannelOrder::Bgr | ChannelOrder::Bgrx | ChannelOrder::Bgra => ChannelOrder::Bgra,
        ChannelOrder::Gray | ChannelOrder::GrayAlpha => ChannelOrder::GrayAlpha,
        ChannelOrder::Xyz | ChannelOrder::XyzA => ChannelOrder::XyzA,
        ChannelOrder::Oklab | ChannelOrder::OklabA => ChannelOrder::OklabA,
        ChannelOrder::Cmyk | ChannelOrder::Cmyka => ChannelOrder::Cmyka,
        ChannelOrder::CmykInverted | ChannelOrder::CmykaInverted => ChannelOrder::CmykaInverted,
        _ => ChannelOrder::Rgba,
    };
    if layout.packing != BitPacking::Full {
        return Err(at!(PipeError::Op(
            "cannot add alpha channel to bit-packed pixel layout without altering source sample grid; convert explicitly before translucent extend_canvas"
                .to_string(),
        )));
    }
    Ok(SampleLayout::new(layout.sample, order))
}

#[allow(clippy::too_many_arguments)]
fn apply_extend_canvas(
    buf: &PixelBuf,
    canvas: Size,
    offset_x: i32,
    offset_y: i32,
    fill_rgba: [u8; 4],
    cs: &ColorServices,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
    pulse: &SharedPulse,
) -> PipeResult<PixelBuf> {
    let orig_fmt = buf.format().clone();
    // Keep existing source samples in their exact original color space and bit depth!
    // Only add an alpha slot in the SAME primaries/transfer/sample-type when the fill
    // is translucent and the source has no alpha channel yet.
    let dst_fmt = if fill_rgba[3] == 255
        || (orig_fmt.layout.order.has_alpha_slot() && orig_fmt.color.alpha() != AlphaMode::None)
    {
        orig_fmt.clone()
    } else {
        PixelFormat::new(
            alpha_capable_layout(orig_fmt.layout)?,
            orig_fmt.color.clone().with_alpha(AlphaMode::Straight),
        )
    };

    let dst_bpp = dst_fmt.layout.bytes_per_pixel();
    let dst_total = check_canvas_limits(canvas.width, canvas.height, dst_bpp, limits)?;

    // Convert ONLY the 1-pixel sRGB fill color into dst_fmt using host ColorServices.
    let mut fill_px = vec![0u8; dst_bpp];
    if dst_fmt == PixelFormat::RGBA8_SRGB {
        fill_px.copy_from_slice(&fill_rgba);
    } else {
        let policy = ConvertPolicy::permissive().with_color_services(cs);
        let fill_plan = RowPlan::build(&PixelFormat::RGBA8_SRGB, &dst_fmt, &policy, None)
            .map_err(|e| at!(PipeError::Op(format!("canvas fill conversion failed: {e}"))))?;
        fill_plan
            .convert_row(&fill_rgba, &mut fill_px, 1)
            .map_err(|e| at!(PipeError::Op(format!("canvas fill conversion failed: {e}"))))?;
    }

    // If an alpha channel had to be appended to an opaque source, convert source within its
    // own color space (no primaries/transfer/gamut change). Otherwise blit source bytes verbatim.
    let promoted_src = if dst_fmt == orig_fmt {
        None
    } else {
        Some(convert_pixel_buf(
            buf, &dst_fmt, cs, false, limits, mem, pulse,
        )?)
    };
    let promoted_bytes = promoted_src.as_ref().map_or(0u64, |b| {
        (b.as_view().row_bytes() as u64).saturating_mul(u64::from(b.height()))
    });

    mem.charge(dst_total as u64)?;
    let px_count = (canvas.width as usize) * (canvas.height as usize);
    let mut raw = Vec::new();
    raw.try_reserve_exact(dst_total).map_err(|_| {
        at!(PipeError::LimitExceeded(
            "extend canvas allocation failed".to_string()
        ))
    })?;
    for _ in 0..px_count {
        raw.extend_from_slice(&fill_px);
    }
    let mut out = PixelBuf::from_bytes(raw, canvas.width, canvas.height, dst_fmt)
        .map_err(|e| at!(PipeError::Op(e.to_string())))?;

    let src_ref = promoted_src.as_ref().unwrap_or(buf);
    let cw = canvas.width as i64;
    let ch = canvas.height as i64;
    let sw = src_ref.width() as i64;
    let sh = src_ref.height() as i64;
    let sx_start = (-i64::from(offset_x)).max(0).min(sw);
    let sx_end = (cw - i64::from(offset_x)).max(0).min(sw);
    let copy_cols = (sx_end - sx_start).max(0) as usize;
    if copy_cols > 0 {
        let sview = src_ref.as_view();
        let mut dview = out.as_view_mut();
        let dx_start = (sx_start + i64::from(offset_x)) as usize;
        let s_off = (sx_start as usize) * dst_bpp;
        let d_off = dx_start * dst_bpp;
        let copy_len = copy_cols * dst_bpp;
        for sy in 0..sh {
            let dy = sy + i64::from(offset_y);
            if dy < 0 || dy >= ch {
                continue;
            }
            let srow = sview.row(sy as u32);
            let drow = dview.row_mut(dy as u32);
            drow[d_off..d_off + copy_len].copy_from_slice(&srow[s_off..s_off + copy_len]);
        }
    }

    if promoted_src.is_some() {
        drop(promoted_src);
        mem.release(promoted_bytes);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn exec_filter<'io>(
    src: &NodeValue<'io>,
    op: &FilterNodeOp,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    #[cfg(not(feature = "std"))]
    {
        let _ = (src, op, cs, engine, run, limits, mem);
        Err(at!(PipeError::Op(
            "canonical filter node requires the 'std' feature (zenfilters)".to_string(),
        )))
    }
    #[cfg(feature = "std")]
    {
        use zenfilters::recipe::{FilterOp as FOp, FilterRecipe};

        let fop = match *op {
            FilterNodeOp::Exposure { stops, .. } => FOp::exposure(stops),
            FilterNodeOp::Contrast { amount, .. } => FOp::contrast(amount),
            FilterNodeOp::Saturation { amount, .. } => FOp::saturation((1.0 + amount).max(0.0)),
            FilterNodeOp::Vibrance { amount, .. } => FOp::vibrance(amount, 0.5),
            FilterNodeOp::Blur { sigma, .. } => FOp::blur(sigma),
            FilterNodeOp::Sharpen { amount, radius, .. } => FOp::sharpen(radius, amount),
            FilterNodeOp::Grayscale { .. } => Ok(FOp::Grayscale),
            FilterNodeOp::Sepia { amount, .. } => FOp::sepia(amount),
        }
        .map_err(|e| at!(PipeError::Op(format!("invalid filter parameter: {e}"))))?;

        let recipe = FilterRecipe::new()
            .with_op(fop)
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        map_frames(src, engine, run, limits, mem, |buf, mem| {
            let src_fmt = buf.format().clone();
            let (primaries, white, has_alpha) = match src_fmt.color {
                ColorSpace::Rgb {
                    primaries,
                    white,
                    alpha,
                    ..
                } => (
                    primaries,
                    white.unwrap_or(DiffuseWhite::BT2408),
                    matches!(alpha, AlphaMode::Straight | AlphaMode::Premultiplied),
                ),
                _ => (Primaries::Bt709, DiffuseWhite::BT2408, false),
            };
            let linear_layout = if has_alpha {
                SampleLayout::RGBAF32
            } else {
                SampleLayout::RGBF32
            };
            let linear_fmt = PixelFormat::new(
                linear_layout,
                ColorSpace::Rgb {
                    primaries,
                    transfer: Transfer::Linear,
                    range: SignalRange::Full,
                    white: Some(white),
                    alpha: if has_alpha {
                        AlphaMode::Straight
                    } else {
                        AlphaMode::None
                    },
                },
            );
            let lin_buf = convert_pixel_buf(buf, &linear_fmt, cs, false, limits, mem, run.pulse())?;
            let lin_size =
                (lin_buf.as_view().row_bytes() as u64).saturating_mul(u64::from(lin_buf.height()));
            let prepared = recipe
                .prepare(primaries, white, buf.width(), buf.height(), has_alpha)
                .map_err(|e| at!(PipeError::Op(format!("filter prepare failed: {e}"))))?;
            let lin_bytes = lin_buf
                .as_view()
                .to_contiguous_bytes()
                .map_err(|e| at!(PipeError::Op(e.to_string())))?;
            let src_f32: &[f32] = bytemuck::cast_slice(&lin_bytes);
            let f32_bytes = (src_f32.len() as u64).saturating_mul(4);
            // Account for `dst_f32` PLUS planar working buffers AND per-filter scratch buffers
            // (e.g. Blur/Sharpen temporary plane + horizontal pass + padded row) BEFORE allocating!
            let scratch_bytes = prepared.total_workspace_bytes();
            mem.check_transient(f32_bytes.saturating_add(scratch_bytes))?;
            let mut dst_f32 = Vec::new();
            dst_f32.try_reserve_exact(src_f32.len()).map_err(|_| {
                at!(PipeError::LimitExceeded(
                    "filter destination buffer allocation failed".to_string()
                ))
            })?;
            dst_f32.resize(src_f32.len(), 0.0f32);
            let mut ctx = zenfilters::FilterContext::new();
            run.pulse()
                .check()
                .map_err(|r| at!(PipeError::Stopped(r)))?;
            prepared
                .apply_with_pulse(src_f32, &mut dst_f32, &mut ctx, None)
                .map_err(|e| at!(PipeError::Op(format!("filter execution failed: {e}"))))?;
            run.pulse()
                .check()
                .map_err(|r| at!(PipeError::Stopped(r)))?;
            drop(ctx);
            drop(lin_bytes);
            drop(lin_buf);
            mem.release(lin_size);
            mem.check_transient(f32_bytes.saturating_mul(2))?;
            mem.charge(f32_bytes)?;
            let mut out_bytes = Vec::new();
            out_bytes
                .try_reserve_exact(dst_f32.len() * 4)
                .map_err(|_| {
                    at!(PipeError::LimitExceeded(
                        "filter output byte buffer allocation failed".to_string()
                    ))
                })?;
            out_bytes.extend_from_slice(bytemuck::cast_slice(&dst_f32));
            drop(dst_f32);
            let filtered_lin =
                PixelBuf::from_bytes(out_bytes, buf.width(), buf.height(), linear_fmt)
                    .map_err(|e| at!(PipeError::Op(e.to_string())))?;
            let result =
                convert_pixel_buf(&filtered_lin, &src_fmt, cs, false, limits, mem, run.pulse())?;
            drop(filtered_lin);
            mem.release(f32_bytes);
            Ok(result)
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn exec_composite<'io>(
    canvas_val: &NodeValue<'io>,
    fg_val: &NodeValue<'io>,
    comp: &CompositeOp,
    cs: &ColorServices,
    engine: &Engine,
    run: &Run<'_>,
    limits: &ResourceLimits,
    mem: &mut LiveMemBudget,
) -> PipeResult<NodeValue<'io>> {
    let fg_buf = materialize_first_still(fg_val, engine, run, mem)?;
    let fg_buf_sz =
        (fg_buf.as_view().row_bytes() as u64).saturating_mul(u64::from(fg_buf.height()));
    let lin_fmt = PixelFormat::RGBAF32_LINEAR;
    let fg_lin = convert_pixel_buf(&fg_buf, &lin_fmt, cs, false, limits, mem, run.pulse())?;
    drop(fg_buf);
    mem.release(fg_buf_sz);
    let fg_lin_sz =
        (fg_lin.as_view().row_bytes() as u64).saturating_mul(u64::from(fg_lin.height()));
    let fw = fg_lin.width() as i64;
    let fh = fg_lin.height() as i64;
    let fg_bytes = fg_lin
        .as_view()
        .to_contiguous_bytes()
        .map_err(|e| at!(PipeError::Op(e.to_string())))?;
    let fg_f32: &[f32] = bytemuck::cast_slice(&fg_bytes);

    let out_val = map_frames(canvas_val, engine, run, limits, mem, |canvas_buf, mem| {
        let orig_fmt = canvas_buf.format().clone();
        let mut dst_lin =
            convert_pixel_buf(canvas_buf, &lin_fmt, cs, false, limits, mem, run.pulse())?;
        let dst_lin_sz =
            (dst_lin.as_view().row_bytes() as u64).saturating_mul(u64::from(dst_lin.height()));
        let cw = dst_lin.width() as i64;
        let ch = dst_lin.height() as i64;
        {
            let mut dst_view = dst_lin.as_view_mut();
            let dst_raw = dst_view.as_contiguous_bytes_mut().ok_or_else(|| {
                at!(PipeError::Op(
                    "composite destination not contiguous".to_string()
                ))
            })?;
            let dst_f32: &mut [f32] = bytemuck::cast_slice_mut(dst_raw);

            for sy in 0..fh {
                let dy = sy + i64::from(comp.y);
                if dy < 0 || dy >= ch {
                    continue;
                }
                for sx in 0..fw {
                    let dx = sx + i64::from(comp.x);
                    if dx < 0 || dx >= cw {
                        continue;
                    }
                    let s0 = ((sy * fw + sx) as usize) * 4;
                    let d0 = ((dy * cw + dx) as usize) * 4;
                    let sr = fg_f32[s0];
                    let sg = fg_f32[s0 + 1];
                    let sb = fg_f32[s0 + 2];
                    let sa = (fg_f32[s0 + 3] * comp.opacity).clamp(0.0, 1.0);

                    let dr = dst_f32[d0];
                    let dg = dst_f32[d0 + 1];
                    let db = dst_f32[d0 + 2];
                    let da = dst_f32[d0 + 3];

                    if comp.blend == BlendKind::Copy {
                        dst_f32[d0] = sr;
                        dst_f32[d0 + 1] = sg;
                        dst_f32[d0 + 2] = sb;
                        dst_f32[d0 + 3] = sa;
                    } else {
                        // Exact W3C Compositing & Blending Level 1 formula:
                        //   C_out_premul = sa*(1-da)*Cs + da*(1-sa)*Cd + sa*da*B(Cs, Cd)
                        //   out_a        = sa + da*(1-sa)
                        // Preserve every positive float alpha (including 2^-24) and handle
                        // exactly-zero alpha explicitly.
                        let (br, bg, bb) = match comp.blend {
                            BlendKind::SourceOver | BlendKind::Copy => (sr, sg, sb),
                            BlendKind::Multiply => (sr * dr, sg * dg, sb * db),
                            BlendKind::Screen => (
                                1.0 - (1.0 - sr) * (1.0 - dr),
                                1.0 - (1.0 - sg) * (1.0 - dg),
                                1.0 - (1.0 - sb) * (1.0 - db),
                            ),
                        };
                        let out_a64 = f64::from(sa) + f64::from(da) * (1.0 - f64::from(sa));
                        let out_a = out_a64 as f32;
                        if out_a64 > 0.0 && out_a > 0.0 {
                            // Divide in f64 so even subnormal positive f32 alphas (e.g. 2^-129,
                            // 2^-149) normalize cleanly without overflowing a 1/out_a reciprocal.
                            let sa64 = f64::from(sa);
                            let da64 = f64::from(da);
                            let w_src = sa64 * (1.0 - da64);
                            let w_dst = da64 * (1.0 - sa64);
                            let w_blend = sa64 * da64;
                            dst_f32[d0] = ((w_src * f64::from(sr)
                                + w_dst * f64::from(dr)
                                + w_blend * f64::from(br))
                                / out_a64) as f32;
                            dst_f32[d0 + 1] = ((w_src * f64::from(sg)
                                + w_dst * f64::from(dg)
                                + w_blend * f64::from(bg))
                                / out_a64) as f32;
                            dst_f32[d0 + 2] = ((w_src * f64::from(sb)
                                + w_dst * f64::from(db)
                                + w_blend * f64::from(bb))
                                / out_a64) as f32;
                            dst_f32[d0 + 3] = out_a;
                        } else {
                            dst_f32[d0] = 0.0;
                            dst_f32[d0 + 1] = 0.0;
                            dst_f32[d0 + 2] = 0.0;
                            dst_f32[d0 + 3] = 0.0;
                        }
                    }
                }
            }
        }
        let converted =
            convert_pixel_buf(&dst_lin, &orig_fmt, cs, false, limits, mem, run.pulse())?;
        drop(dst_lin);
        mem.release(dst_lin_sz);
        Ok(converted)
    });
    drop(fg_bytes);
    drop(fg_lin);
    mem.release(fg_lin_sz);
    out_val
}
