//! Canonical codec planner and executor (`Engine`, `Run`, `OutputIntent`, and `RouteReport`).
//!
//! Built around one immutable [`zencodec::CodecSet`] (rejecting pre-stamped operation
//! defaults), one [`Routing`] policy, one frozen [`zenpixels_convert::ColorServices`]
//! binding, and one [`zencodec::JobResources`] budget/cancellation chain.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ENGINE_ID: AtomicUsize = AtomicUsize::new(1);

use enough::StopReason;
use whereat::{At, at};
use zencodec::decode::{DecodeOutput, DynDecoderConfig};
use zencodec::encode::{EncodeOutput, Fidelity};
use zencodec::operation::{OperationReport, PolicySet};
use zencodec::source::NativeBound;
use zencodec::storage::StorageBudget;
use zencodec::time::FrameDuration;
use zencodec::{
    CodecSet, ColorEmitPolicy, EncodedTransformRole, ImageDecodeRole, ImageEncodeRole, ImageFormat,
    ImageInfo, JobResources, Metadata, MetadataFields, RegistrationId, ResourceLimits, SharedPulse,
};
use zenpixels::{
    AlphaMode, BitPacking, ColorSpace, DiffuseWhite, HlgRendering, Nits, Orientation, PeakSource,
    PixelBuf, PixelFormat, PixelView, Primaries, SampleLayout, SignalRange, Transfer,
};
use zenpixels_convert::{
    CmsChoice, ColorServices, ConvertPolicy, OutputPlan, OutputSignaling, RowPlan,
};

#[cfg(feature = "media")]
use zencodec_media::registry::ContainerId;
#[cfg(feature = "media")]
use zencodec_media::track::CodecId as MediaCodecId;

use crate::routing::{Routing, RoutingError, RuleKey};

// ═══════════════════════════════════════════════════════════════════════════
// Canonical portable request & intent vocabulary
// ═══════════════════════════════════════════════════════════════════════════

/// Ordered non-empty list of acceptable target image formats for [`TargetFormat::Auto`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatPreferences {
    ordered: Vec<ImageFormat>,
}

impl FormatPreferences {
    /// Construct an ordered format preference list; rejects empty lists.
    pub fn ordered(formats: impl IntoIterator<Item = ImageFormat>) -> EngineResult<Self> {
        let mut out = Vec::new();
        for f in formats {
            if !out.contains(&f) {
                out.push(f);
            }
        }
        if out.is_empty() {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "FormatPreferences requires at least one format",
            )));
        }
        Ok(Self { ordered: out })
    }

    /// Borrow the ordered format list.
    #[must_use]
    pub fn formats(&self) -> &[ImageFormat] {
        &self.ordered
    }
}

/// Target format selection for encoding or transcoding.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// Encode to this exact still/animated image format.
    Exact(ImageFormat),
    /// Preserve the source's primary-lineage image format.
    Keep,
    /// Select the first admissible format from an explicit non-empty preference list.
    Auto(FormatPreferences),
    /// Encode to a video container + elementary bitstream codec tuple.
    #[cfg(feature = "media")]
    Video {
        /// Output media container (MP4, WebM, IVF, or custom).
        container: ContainerId,
        /// Output video bitstream codec (AV1, VP8, VP9, etc.).
        codec: MediaCodecId,
    },
}

/// Portable encoder compute effort tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Effort {
    /// Lowest latency / fastest compression.
    Fast,
    /// Balanced speed and compression efficiency.
    #[default]
    Balanced,
    /// Maximum compression efficiency within budget.
    Thorough,
}

/// Portable perceptual quality profile tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QualityLevel {
    /// Aggressive size reduction suitable for thumbnails/previews.
    Low,
    /// Good web delivery quality.
    #[default]
    Good,
    /// High fidelity photographic delivery.
    High,
    /// Near-visually-lossless delivery.
    VisuallyLossless,
}

impl QualityLevel {
    /// Frozen calibrated 0–100 quality mapping for this profile level and DPR.
    #[must_use]
    pub fn calibrated_quality(self, dpr: f32) -> f32 {
        let base: f32 = match self {
            Self::Low => 62.0,
            Self::Good => 82.0,
            Self::High => 90.0,
            Self::VisuallyLossless => 96.0,
        };
        let dpr_adj = if dpr.is_finite() && dpr > 1.0 {
            ((dpr - 1.0).min(2.0)) * 4.0
        } else {
            0.0
        };
        (base - dpr_adj).clamp(5.0, 100.0)
    }
}

/// Supported scalar perceptual quality metrics for [`Coding::AimFor`] and [`Coding::Verify`].
///
/// Score-only scalar APIs only; diagnostic diffmaps/heatmaps are never generated by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Metric {
    /// SSIMULACRA2 (higher is better, typical range `<= 100.0`).
    Ssimulacra2,
    /// Butteraugli max distance (lower is better, `>= 0.0`).
    ButteraugliMax,
    /// Zensim Profile A score (higher is better, `<= 100.0`).
    ZensimA,
    /// Zensim Profile B score (higher is better, `<= 100.0`).
    ZensimB,
}

/// Revision token for the one-shot heuristic seed estimate used by [`Coding::AimFor`].
///
/// [`Coding::AimFor`] maps a metric target to a single encoder quality seed without running
/// closed-loop verification. Hosts pinning a revision on `Coding::AimFor` must match this
/// identifier (or use [`Coding::Verify`] for closed-loop measured convergence).
pub const AIM_SEED_REVISION: &str = "zencodecs.aim_seed.v1";

impl Metric {
    /// Canonical lowercase wire token shared across `zencodecs`, `zenpipe`, and `imageflow`.
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Ssimulacra2 => "ssimulacra2",
            Self::ButteraugliMax => "butteraugli_max",
            Self::ZensimA => "zensim_a",
            Self::ZensimB => "zensim_b",
        }
    }

    /// Whether higher scalar values indicate better visual fidelity.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        match self {
            Self::Ssimulacra2 | Self::ZensimA | Self::ZensimB => true,
            Self::ButteraugliMax => false,
        }
    }

    /// Ideal score for two identical images.
    #[must_use]
    pub const fn perfect_score(self) -> f64 {
        if self.higher_is_better() { 100.0 } else { 0.0 }
    }

    /// Validate that `value` is finite and lies within the metric's mathematical domain.
    pub fn validate_target_value(self, value: f32) -> EngineResult<()> {
        if !value.is_finite() {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "metric target value must be finite",
            )));
        }
        let valid = match self {
            Self::Ssimulacra2 | Self::ZensimA | Self::ZensimB => value <= 100.0,
            Self::ButteraugliMax => value >= 0.0,
        };
        if !valid {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "metric target value is outside metric domain",
            )));
        }
        Ok(())
    }

    /// Check whether `observed` satisfies `target` (f32 compatibility helper).
    #[must_use]
    pub fn satisfies(self, observed: f32, target: f32) -> bool {
        self.satisfies_f64(f64::from(observed), f64::from(target))
    }

    /// Check whether native `f64` `observed` satisfies `target` without `f32` rounding drift.
    #[must_use]
    pub fn satisfies_f64(self, observed: f64, target: f64) -> bool {
        if !observed.is_finite() || !target.is_finite() {
            return false;
        }
        if self.higher_is_better() {
            observed >= target
        } else {
            observed <= target
        }
    }
}

/// Validated scalar metric target for calibrated aim or closed-loop verification.
///
/// Target thresholds are stored as `f32` on the portable request struct; closed-loop
/// verification compares native `f64` scorer observations against `f64::from(self.value)`
/// to prevent narrowing false passes.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricTarget {
    /// Metric identifier.
    pub metric: Metric,
    /// Optional pinned scorer or calibration revision string.
    pub revision: Option<String>,
    /// Target scalar value (finite, within metric domain).
    pub value: f32,
}

impl MetricTarget {
    /// Construct a validated metric target.
    pub fn new(metric: Metric, value: f32) -> EngineResult<Self> {
        metric.validate_target_value(value)?;
        Ok(Self {
            metric,
            revision: None,
            value,
        })
    }

    /// Re-validate domain and pinned revision consistency (guards struct literal construction).
    pub fn validate(&self) -> EngineResult<()> {
        self.metric.validate_target_value(self.value)?;
        if let Some(ref rev) = self.revision
            && rev.trim().is_empty()
        {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "metric target revision pin must not be empty",
            )));
        }
        Ok(())
    }

    /// Pin an exact scorer revision string.
    #[must_use]
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }
}

/// Stable identity metadata for a registered [`MetricScorer`] (portable mirror of `zenpsy-api`'s `MetricId`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricScorerMetadata {
    /// Metric family (e.g. `"zensim"`, `"ssimulacra2"`, `"butteraugli"`).
    pub family: &'static str,
    /// Optional variant within the family (e.g. `Some("zensim-a")`).
    pub variant: Option<&'static str>,
    /// Reproduced reference specification or model weight provenance (empty when unset).
    pub reference: String,
    /// Implementing crate name (`CARGO_PKG_NAME`).
    pub crate_name: &'static str,
    /// Implementing crate version (`CARGO_PKG_VERSION`).
    pub crate_version: &'static str,
    /// Numeric score definition revision (distinct from crate version).
    pub score_revision: u32,
    /// Compute backend label (`"cpu"` or GPU API name).
    pub backend: &'static str,
    /// Optional build identifier.
    pub build: Option<&'static str>,
    /// Sorted score-affecting configuration key/value pairs.
    pub config: Vec<(String, String)>,
}

/// Job-local prepared perceptual metric scorer holding a precomputed reference and reusable
/// scratch across repeated candidate trials in [`Coding::Verify`].
///
/// Strictly score-only; never allocates diagnostic diffmaps.
pub trait PreparedMetricScorer: Send {
    /// Score `candidate` (in the parent scorer's `comparison_format()`) against the
    /// prepared reference, returning the native `f64` scalar score.
    fn score_candidate_f64(
        &mut self,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64>;
}

/// Score-only scalar perceptual metric provider for [`Coding::Verify`] and direct comparison.
///
/// Only scalar scores are returned; diffmaps and diagnostic arrays are never produced.
pub trait MetricScorer: Send + Sync {
    /// Metric computed by this scorer.
    fn metric(&self) -> Metric;
    /// Stable revision identifier recorded in reports and cache keys.
    fn revision(&self) -> &'static str;
    /// Pixel format expected for both `reference` and `candidate` rasters.
    fn comparison_format(&self) -> PixelFormat;
    /// Score `candidate` against `reference` (both in `comparison_format()`).
    fn score(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f32>;

    /// Score `candidate` against `reference`, returning the native `f64` scalar score.
    ///
    /// Default widens [`Self::score`] to `f64` for compatibility with legacy scorers.
    fn score_f64(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        self.score(reference, candidate, resources).map(f64::from)
    }

    /// Optionally prepare `reference` once for repeated candidate scoring during [`Coding::Verify`].
    ///
    /// Returning `Ok(None)` falls back to cold per-trial scoring via [`Self::score_f64`].
    fn prepare_reference(
        &self,
        _reference: PixelView<'_>,
        _resources: &JobResources,
    ) -> EngineResult<Option<Box<dyn PreparedMetricScorer>>> {
        Ok(None)
    }

    /// Conservative upper bound on bytes retained by prepared reference state plus scoring scratch.
    fn estimated_scratch_bytes(&self, width: u32, height: u32) -> u64 {
        u64::from(width.max(8))
            .saturating_mul(u64::from(height.max(8)))
            .saturating_mul(16)
    }

    /// Optional structured identity metadata mirroring `zenpsy-api::MetricId`.
    fn metadata(&self) -> Option<MetricScorerMetadata> {
        None
    }
}

/// Portable encoding fidelity / coding mode.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum Coding {
    /// Use calibrated balanced quality for lossy formats or registered lossless default.
    #[default]
    Auto,
    /// Calibrated perceptual quality on the canonical `0.0..=100.0` scale.
    CalibratedQuality(f32),
    /// Named perceptual profile adjusted for device pixel ratio (`dpr`).
    QualityProfile {
        /// Quality tier.
        level: QualityLevel,
        /// Target device pixel ratio (e.g. `1.0`, `2.0`).
        dpr: f32,
    },
    /// One-shot heuristic estimate targeting a scalar perceptual metric (no verification loop).
    ///
    /// Uses the pinned clamped seed heuristic (`AIM_SEED_REVISION = "zencodecs.aim_seed.v1"`),
    /// recording requested metric, target, revision, and estimated quality in [`RouteReport`]
    /// without measuring the encoded output. Use [`Coding::Verify`] for closed-loop verification.
    AimFor(MetricTarget),
    /// Closed-loop verified still-SDR encode converging on a scalar metric target.
    Verify(MetricTarget),
    /// Mathematically lossless pixel preservation.
    Lossless,
    /// Lossless when source is known lossless, Good calibrated quality when source is lossy.
    LosslessOrKeep,
    /// Byte-exact passthrough (requires same format and compatible metadata/orientation policy).
    KeepBytes,
    /// Byte-exact JPEG recovery across JPEG ↔ JXL reconstruction routes.
    ExactJpegRoundTrip,
}

/// Chroma subsampling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromaSampling {
    /// 4:4:4 (no chroma subsampling).
    Cs444,
    /// 4:2:2 (horizontal chroma subsampling).
    Cs422,
    /// 4:2:0 (horizontal and vertical chroma subsampling).
    Cs420,
}

/// Chroma handling intent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromaPolicy {
    /// Let the encoder choose based on quality/format.
    #[default]
    Auto,
    /// Preserve known source subsampling where representable.
    Preserve,
    /// Require an exact coded chroma grid.
    Require(ChromaSampling),
}

/// Alpha handling intent at encode time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlphaPolicy {
    /// Preserve alpha if present; refuse opaque-only target when alpha is non-opaque.
    #[default]
    Preserve,
    /// Require input to be opaque (refuse non-opaque alpha).
    RequireOpaque,
    /// Composite alpha onto an sRGB matte color in linear light.
    FlattenSrgb([u8; 3]),
}

/// Embedded orientation handling at decode ingress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeOrientation {
    /// Apply embedded EXIF/container D4 orientation once to upright samples.
    #[default]
    Apply,
    /// Keep stored sample order and retain pending embedded orientation metadata.
    KeepStored,
    /// Ignore embedded orientation metadata.
    Ignore,
}

/// Orientation emission mode at encode egress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OrientationMode {
    /// Bake display orientation into upright pixels and emit identity orientation.
    #[default]
    Bake,
    /// Preserve stored sample order and require an orientation carrier tag.
    PreserveStored,
}

/// Metadata retention policy distinguishing semantic `Preserve` from byte-exact `PreserveBytes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetadataPolicy {
    /// Web delivery default: keep display-critical fields and supported rights, drop tracking EXIF.
    #[default]
    Web,
    /// Semantic preservation: keep descriptive metadata and allow required structural rewrites
    /// (for example reconciling EXIF orientation after baking rotation).
    Preserve,
    /// Byte-exact preservation: refuse if ANY rewrite is required (including EXIF orientation
    /// rewrite) or if the target container lacks a faithful carrier.
    PreserveBytes,
    /// Keep only color signaling and orientation metadata.
    ColorAndRotation,
    /// Strip all optional descriptive metadata while keeping truthful color/dimension carriers.
    StripAll,
    /// Explicit per-field retention bitmask.
    Custom(MetadataFields),
}

impl MetadataPolicy {
    /// Lower to `zencodec::MetadataPolicy` for codec-level metadata filtering.
    #[must_use]
    pub fn to_zencodec(self) -> zencodec::MetadataPolicy {
        match self {
            Self::Web => zencodec::MetadataPolicy::Web,
            Self::Preserve | Self::PreserveBytes => zencodec::MetadataPolicy::PreserveExact,
            Self::ColorAndRotation => zencodec::MetadataPolicy::ColorAndRotation,
            Self::StripAll => zencodec::MetadataPolicy::Custom(MetadataFields::DISCARD_ALL),
            Self::Custom(fields) => zencodec::MetadataPolicy::Custom(fields),
        }
    }

    /// Whether this policy forbids any rewrite of retained descriptive payloads (including EXIF orientation).
    #[must_use]
    pub const fn requires_byte_exact_payloads(self) -> bool {
        matches!(self, Self::PreserveBytes)
    }
}

/// Animation / video timing exactness policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimingPolicy {
    /// Require exact rational duration representation (refuse quantization).
    #[default]
    Exact,
    /// Quantize cumulative endpoints with ties-toward-positive-infinity, bounded by `max_boundary_error`.
    QuantizeEndpoints {
        /// Maximum allowed timing error at any frame boundary (including total duration).
        max_boundary_error: FrameDuration,
    },
}

/// Normalized loop-count fact or target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoopCount {
    /// Carrier did not specify a loop count.
    #[default]
    Unspecified,
    /// Loop forever.
    Infinite,
    /// Total play count including the first presentation (`1` = play once, no repeat).
    Plays(NonZeroU32),
}

impl LoopCount {
    /// Play once (single pass — canonical for video containers without repeat metadata).
    #[must_use]
    pub const fn once() -> Self {
        Self::Plays(NonZeroU32::MIN)
    }
}

/// Loop-count handling policy across animation and video transcodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoopPolicy {
    /// Preserve source loop count (refuses when destination container cannot represent it).
    #[default]
    Preserve,
    /// Explicitly override output loop count (e.g. `LoopCount::once()` for animation -> video).
    Set(LoopCount),
}

/// Multi-frame / sequence preservation policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SequencePolicy {
    /// Preserve all presentations with the specified timing and loop policies.
    Preserve {
        /// Exact or bounded endpoint quantization timing rule.
        timing: TimingPolicy,
        /// Loop preservation or explicit override.
        loops: LoopPolicy,
    },
    /// Explicitly select only the first presentation (still poster).
    FirstPresentation,
}

impl Default for SequencePolicy {
    fn default() -> Self {
        Self::Preserve {
            timing: TimingPolicy::Exact,
            loops: LoopPolicy::Preserve,
        }
    }
}

/// Non-video track disposition when transcoding media containers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrackPolicy {
    /// Preserve all compatible tracks; refuse if destination cannot carry present audio/side tracks.
    #[default]
    Preserve,
    /// Explicitly select one video track and optional side tracks to stream-copy.
    Select {
        /// Zero-based video track index.
        video: u32,
        /// Additional track indices to copy verbatim.
        copy: Vec<u32>,
    },
    /// Explicitly drop non-video tracks (required when converting audio-bearing video to still/animation/IVF).
    DropNonVideo,
}

/// Tone-mapping operator choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToneMapMethod {
    /// ITU-R BT.2408 EETF.
    #[default]
    Bt2408,
    /// ITU-R BT.2446 Method A.
    Bt2446A,
    /// ITU-R BT.2446 Method B.
    Bt2446B,
    /// ITU-R BT.2446 Method C.
    Bt2446C,
}

/// Gamut mapping mode during tone mapping or color conversion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GamutMode {
    /// Preserve source gamut primaries where representable.
    #[default]
    Preserve,
    /// Hard clip out-of-gamut values to destination gamut.
    Clip,
    /// Soft perceptual gamut compression.
    Compress,
}

/// Complete HDR-to-SDR (or display-referred) tone-map render specification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneMap {
    /// Tone-mapping method.
    pub method: ToneMapMethod,
    /// Target primaries.
    pub target_primaries: Primaries,
    /// Source peak acquisition mode.
    pub source_peak: PeakSource,
    /// Reference diffuse white in cd/m².
    pub white: DiffuseWhite,
    /// Target display peak luminance in cd/m².
    pub target_peak: Nits,
    /// Optional HLG display rendering parameters.
    pub hlg: Option<HlgRendering>,
    /// Gamut handling.
    pub gamut: GamutMode,
}

impl ToneMap {
    /// Construct a validated SDR render specification.
    pub fn sdr(
        method: ToneMapMethod,
        source_peak: PeakSource,
        white: DiffuseWhite,
        target_peak: Nits,
    ) -> EngineResult<Self> {
        if !target_peak.nits().is_finite()
            || target_peak.nits() <= 0.0
            || white.nits() > target_peak.nits()
        {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "tone-map target_peak must be positive finite and >= diffuse white",
            )));
        }
        Ok(Self {
            method,
            target_primaries: Primaries::Bt709,
            source_peak,
            white,
            target_peak,
            hlg: None,
            gamut: GamutMode::Preserve,
        })
    }

    /// Set target primaries.
    #[must_use]
    pub const fn with_target_primaries(mut self, primaries: Primaries) -> Self {
        self.target_primaries = primaries;
        self
    }

    /// Set HLG display rendering parameters.
    #[must_use]
    pub const fn with_hlg(mut self, hlg: HlgRendering) -> Self {
        self.hlg = Some(hlg);
        self
    }

    /// Set gamut mapping mode.
    #[must_use]
    pub const fn with_gamut(mut self, gamut: GamutMode) -> Self {
        self.gamut = gamut;
        self
    }
}

/// Tone-mapping authorization at encode time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum ToneMapPolicy {
    /// Require an HDR-capable destination for HDR input (no implicit SDR flattening).
    #[default]
    RequireCompatible,
    /// Explicitly authorize tone mapping to SDR using the supplied render specification.
    MapToSdr(ToneMap),
}

/// Target rendition when reconstructing from a gain map.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum GainMapRendition {
    /// Base rendition (no gain applied).
    Base,
    /// Alternate endpoint rendition.
    Alternate,
    /// Target display headroom in stops (`log2(peak / white)`).
    DisplayHeadroomStops(f32),
}

/// Spatial registration fidelity when carrying a gain map through transforms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GainMapFidelity {
    /// Allow only exact registration (identity, D4, or phase-exact lattice crop).
    #[default]
    ExactRegistration,
    /// Explicitly permit resampling the gain map to match output geometry.
    Resample,
}

/// Gain-map processing action.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum GainMapAction {
    /// Keep only the base rendition and drop the auxiliary gain map.
    BaseOnly,
    /// Reconstruct a single rendition from base + gain map.
    Reconstruct {
        /// Desired output rendition / headroom.
        rendition: GainMapRendition,
    },
    /// Carry the base + gain map pair together.
    Carry {
        /// Registration fidelity requirement.
        fidelity: GainMapFidelity,
    },
}

/// Gain-map policy at encode time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GainMapEncodePolicy {
    /// Preserve a valid gain map if present and representable; refuse silent drop.
    #[default]
    Preserve,
    /// Require a gain map to be present and encoded.
    Require,
    /// Explicitly drop any gain map (keep base only).
    Drop,
}

/// Enforcement strictness for format-level [`CodecOptions`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OptionMode {
    /// Warn on unsupported keys without failing selection.
    #[default]
    Advisory,
    /// Exclude candidates that cannot honor all declared options.
    Strict,
}

/// Typed scalar option value inside versioned format [`CodecOptions`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum OptionValue {
    /// Boolean option (e.g. `progressive: true`).
    Bool(bool),
    /// Signed 64-bit integer option.
    Int(i64),
    /// Finite floating-point option.
    Float(f64),
    /// Short string token option.
    Text(String),
}

/// Versioned format-level bitstream options (never backend crate types).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodecOptions {
    /// Enforcement strictness (`Advisory` default).
    pub mode: OptionMode,
    /// Key-value options keyed by versioned format namespace (e.g. `"org.imazen.jpeg/v1"`).
    pub namespaces: BTreeMap<String, BTreeMap<String, OptionValue>>,
}

impl CodecOptions {
    /// Create empty advisory options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set strictness mode.
    #[must_use]
    pub const fn with_mode(mut self, mode: OptionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Insert a typed option in a versioned namespace.
    pub fn set(
        mut self,
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: OptionValue,
    ) -> Self {
        self.namespaces
            .entry(namespace.into())
            .or_default()
            .insert(key.into(), value);
        self
    }
}

/// Output publication schedule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PublicationMode {
    /// Stage complete output privately, verify postconditions, then publish.
    #[default]
    Verified,
    /// Stream verified byte chunks incrementally before EOF (forbids post-EOF verification).
    Incremental,
}

/// Target pixel color policy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorTarget {
    /// Preserve active pixel color space where representable.
    #[default]
    Preserve,
    /// Convert to the specified pixel format before encoding.
    ConvertTo(PixelFormat),
}

/// Handling of inputs with genuinely unspecified color space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnspecifiedColor {
    /// Refuse unspecified color if conversion requires a known space.
    #[default]
    Refuse,
    /// Assume sRGB for genuinely unspecified RGB/gray samples (recorded in trace).
    AssumeSrgb,
}

/// Complete portable encode/transcode intent.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputIntent {
    /// Target format or video container+codec tuple.
    pub target: TargetFormat,
    /// Quality / lossless / metric / passthrough mode.
    pub coding: Coding,
    /// Compute effort tier.
    pub effort: Effort,
    /// Chroma subsampling intent.
    pub chroma: ChromaPolicy,
    /// Alpha handling intent.
    pub alpha: AlphaPolicy,
    /// Orientation mode at egress.
    pub orientation: OrientationMode,
    /// Target pixel color space.
    pub color: ColorTarget,
    /// Handling of genuinely unspecified source color.
    pub unspecified_color: UnspecifiedColor,
    /// Color carrier emission policy.
    pub color_emit: ColorEmitPolicy,
    /// HDR tone-mapping authorization.
    pub tone_map: ToneMapPolicy,
    /// Gain-map policy.
    pub gain_map: GainMapEncodePolicy,
    /// Metadata retention policy (`Preserve` vs `PreserveBytes` vs `Web` etc.).
    pub metadata: MetadataPolicy,
    /// Sequence / timing / loop preservation policy.
    pub sequence: SequencePolicy,
    /// Non-video track disposition (audio/subtitles/side tracks).
    pub tracks: TrackPolicy,
    /// Publication schedule (`Verified` vs `Incremental`).
    pub publication: PublicationMode,
    /// Versioned format-level bitstream options.
    pub codec_options: CodecOptions,
}

impl OutputIntent {
    /// Construct an [`OutputIntent`] with conservative, truthful defaults:
    /// `Auto` coding, `Balanced` effort, `Preserve` alpha/color/HDR/gain-map/sequence,
    /// `Bake` orientation, `MetadataPolicy::Web`, `PublicationMode::Verified`.
    #[must_use]
    pub fn new(target: TargetFormat) -> Self {
        Self {
            target,
            coding: Coding::Auto,
            effort: Effort::Balanced,
            chroma: ChromaPolicy::Auto,
            alpha: AlphaPolicy::Preserve,
            orientation: OrientationMode::Bake,
            color: ColorTarget::Preserve,
            unspecified_color: UnspecifiedColor::Refuse,
            color_emit: ColorEmitPolicy::Balanced,
            tone_map: ToneMapPolicy::RequireCompatible,
            gain_map: GainMapEncodePolicy::Preserve,
            metadata: MetadataPolicy::Web,
            sequence: SequencePolicy::default(),
            tracks: TrackPolicy::Preserve,
            publication: PublicationMode::Verified,
            codec_options: CodecOptions::new(),
        }
    }

    /// Convenience constructor targeting an exact image format.
    #[must_use]
    pub fn format(format: ImageFormat) -> Self {
        Self::new(TargetFormat::Exact(format))
    }

    /// Convenience constructor targeting a video container and codec tuple.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn video(container: ContainerId, codec: MediaCodecId) -> Self {
        Self::new(TargetFormat::Video { container, codec })
    }

    /// Set coding mode.
    #[must_use]
    pub fn with_coding(mut self, coding: Coding) -> Self {
        self.coding = coding;
        self
    }

    /// Set compute effort tier.
    #[must_use]
    pub const fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = effort;
        self
    }

    /// Set chroma handling policy.
    #[must_use]
    pub const fn with_chroma(mut self, chroma: ChromaPolicy) -> Self {
        self.chroma = chroma;
        self
    }

    /// Set alpha handling policy.
    #[must_use]
    pub const fn with_alpha(mut self, alpha: AlphaPolicy) -> Self {
        self.alpha = alpha;
        self
    }

    /// Set output orientation mode.
    #[must_use]
    pub const fn with_orientation(mut self, orientation: OrientationMode) -> Self {
        self.orientation = orientation;
        self
    }

    /// Set target pixel color space.
    #[must_use]
    pub fn with_color(mut self, color: ColorTarget) -> Self {
        self.color = color;
        self
    }

    /// Set unspecified-color handling.
    #[must_use]
    pub const fn with_unspecified_color(mut self, mode: UnspecifiedColor) -> Self {
        self.unspecified_color = mode;
        self
    }

    /// Set color carrier emission policy.
    #[must_use]
    pub const fn with_color_emit(mut self, emit: ColorEmitPolicy) -> Self {
        self.color_emit = emit;
        self
    }

    /// Set HDR tone-map policy.
    #[must_use]
    pub const fn with_tone_map(mut self, tone_map: ToneMapPolicy) -> Self {
        self.tone_map = tone_map;
        self
    }

    /// Set gain-map encode policy.
    #[must_use]
    pub const fn with_gain_map(mut self, gain_map: GainMapEncodePolicy) -> Self {
        self.gain_map = gain_map;
        self
    }

    /// Set metadata retention policy.
    #[must_use]
    pub const fn with_metadata(mut self, metadata: MetadataPolicy) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set sequence / timing / loop preservation policy.
    #[must_use]
    pub const fn with_sequence(mut self, sequence: SequencePolicy) -> Self {
        self.sequence = sequence;
        self
    }

    /// Set non-video track policy.
    #[must_use]
    pub fn with_tracks(mut self, tracks: TrackPolicy) -> Self {
        self.tracks = tracks;
        self
    }

    /// Set publication schedule (`Verified` vs `Incremental`).
    #[must_use]
    pub const fn with_publication(mut self, publication: PublicationMode) -> Self {
        self.publication = publication;
        self
    }

    /// Set versioned format-level codec options.
    #[must_use]
    pub fn with_codec_options(mut self, options: CodecOptions) -> Self {
        self.codec_options = options;
        self
    }

    /// Validate internal semantic consistency before touching any input bytes.
    pub fn validate(&self) -> EngineResult<()> {
        match &self.coding {
            Coding::CalibratedQuality(q) => {
                if !q.is_finite() || !(0.0..=100.0).contains(q) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "calibrated_quality must be finite in 0..=100",
                    )));
                }
            }
            Coding::QualityProfile { dpr, .. } => {
                if !dpr.is_finite() || *dpr <= 0.0 {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "quality_profile dpr must be positive finite",
                    )));
                }
            }
            Coding::Lossless => {
                if matches!(self.alpha, AlphaPolicy::FlattenSrgb(_)) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Lossless conflicts with alpha matte flattening",
                    )));
                }
                if matches!(self.tone_map, ToneMapPolicy::MapToSdr(_)) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Lossless conflicts with HDR tone mapping",
                    )));
                }
                if matches!(
                    self.chroma,
                    ChromaPolicy::Require(ChromaSampling::Cs420 | ChromaSampling::Cs422)
                ) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Lossless conflicts with subsampled chroma requirement",
                    )));
                }
            }
            Coding::AimFor(target) => {
                target.validate()?;
            }
            Coding::Verify(target) => {
                target.validate()?;
                if matches!(self.publication, PublicationMode::Incremental) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Verify requires PublicationMode::Verified (cannot stream before scoring)",
                    )));
                }
            }
            Coding::ExactJpegRoundTrip => {
                if matches!(self.publication, PublicationMode::Incremental) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "ExactJpegRoundTrip requires PublicationMode::Verified",
                    )));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Probe depth for [`Engine::inspect`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ProbeDepth {
    /// Fast magic and header inspection.
    #[default]
    Header,
    /// Full header interpretation (dimensions, color authority, sequence/track facts).
    Interpretation,
    /// Structural container inventory scan.
    Inventory,
}

/// Request for [`Engine::inspect`].
#[derive(Clone, Debug)]
pub struct Inspect {
    /// Requested depth.
    pub depth: ProbeDepth,
    /// Optional expected format assertion.
    pub expected_format: Option<ImageFormat>,
}

impl Inspect {
    /// Create a new inspection request at `depth`.
    #[must_use]
    pub const fn new(depth: ProbeDepth) -> Self {
        Self {
            depth,
            expected_format: None,
        }
    }

    /// Require detected format to match `format`.
    #[must_use]
    pub const fn with_expected_format(mut self, format: ImageFormat) -> Self {
        self.expected_format = Some(format);
        self
    }
}

/// Media cardinality kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaKind {
    /// Single still image.
    Still,
    /// Timed animation (GIF, APNG, WebP, animated AVIF/JXL).
    Animation,
    /// Multi-page document/collection (PDF, multi-page TIFF).
    Pages,
    /// Video container stream (MP4, WebM, IVF).
    Video,
}

/// Summary of an inspected image or media stream.
#[derive(Clone, Debug)]
pub struct MediaEnvelope {
    /// High-level media cardinality.
    pub kind: MediaKind,
    /// Detected still/animated image format, if applicable.
    pub image_format: Option<ImageFormat>,
    /// Detected video container, if applicable.
    #[cfg(feature = "media")]
    pub container: Option<ContainerId>,
    /// Canvas width in pixels (0 if unknown before full track decode).
    pub width: u32,
    /// Canvas height in pixels (0 if unknown before full track decode).
    pub height: u32,
    /// Whether the media carries alpha.
    pub has_alpha: bool,
    /// Known frame/page count, if established.
    pub frame_count: Option<u32>,
    /// Embedded orientation fact (default `Identity`).
    pub orientation: Orientation,
    /// Whether a gain map is present.
    pub has_gain_map: bool,
    /// Whether non-video tracks (e.g. audio) were discovered.
    pub has_non_video_tracks: bool,
}

impl MediaEnvelope {
    /// Build a [`MediaEnvelope`] from an [`ImageInfo`].
    #[must_use]
    pub fn from_image_info(info: &ImageInfo) -> Self {
        let (kind, frame_count) = match info.sequence {
            zencodec::ImageSequence::Single => (MediaKind::Still, Some(1)),
            zencodec::ImageSequence::Animation { frame_count, .. } => {
                (MediaKind::Animation, frame_count)
            }
            zencodec::ImageSequence::Multi { image_count, .. } => (MediaKind::Pages, image_count),
            _ => (MediaKind::Still, None),
        };
        Self {
            kind,
            image_format: Some(info.format),
            #[cfg(feature = "media")]
            container: None,
            width: info.width,
            height: info.height,
            has_alpha: info.has_alpha,
            frame_count,
            orientation: info.orientation,
            has_gain_map: info.supplements.gain_map,
            has_non_video_tracks: false,
        }
    }
}

/// Result of [`Engine::inspect`].
#[derive(Clone, Debug)]
pub struct Inspection {
    /// High-level media envelope.
    pub envelope: MediaEnvelope,
    /// Image header metadata when the input is an image format.
    pub image_info: Option<ImageInfo>,
    /// Provider registration used for inspection, if a registered backend probed it.
    pub probed_by: Option<RegistrationId>,
}

/// Decode request options.
#[derive(Clone, Debug, Default)]
pub struct DecodeNeeds {
    /// Optional preferred output pixel format.
    pub output_format: Option<PixelFormat>,
    /// Embedded orientation handling.
    pub orientation: DecodeOrientation,
    /// Explicit single-presentation selection from a sequence/collection.
    pub select_primary: bool,
    /// Whether to extract an embedded gain map alongside the base image.
    pub extract_gain_map: bool,
}

impl DecodeNeeds {
    /// Require a single still image (refuses multi-frame animations unless `select_primary` is set).
    #[must_use]
    pub fn still() -> Self {
        Self::default()
    }

    /// Explicitly select the primary presentation from a sequence or collection.
    #[must_use]
    pub fn primary() -> Self {
        Self {
            select_primary: true,
            ..Self::default()
        }
    }

    /// Request conversion to `format`.
    #[must_use]
    pub fn with_output(mut self, format: PixelFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Configure embedded orientation handling.
    #[must_use]
    pub const fn with_orientation(mut self, orientation: DecodeOrientation) -> Self {
        self.orientation = orientation;
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Errors, traces, route reports, and completion receipts
// ═══════════════════════════════════════════════════════════════════════════

/// Pipeline stage at which an engine operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Request validation and container/format probing.
    Inspect,
    /// Routing permission, capability, and resource bound admission.
    Admit,
    /// Job/session/converter preparation.
    Prepare,
    /// Pixel/packet decoding, conversion, and encoding execution.
    Execute,
    /// Postcondition or perceptual metric verification.
    Verify,
    /// Output staging commit / publication.
    Publish,
}

/// Matchable failure classification for [`EngineError`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// Caller request is internally contradictory or out of domain.
    InvalidRequest,
    /// Input bytes did not match any recognized image or media container format.
    UnrecognizedFormat,
    /// Input or output format is recognized but denied by [`Routing`].
    DeniedFormat(ImageFormat),
    /// Recognized format has no eligible registered backend in [`CodecSet`].
    NoBackend(ImageFormat),
    /// Media container or video codec is denied or has no eligible registered factory.
    NoMediaBackend,
    /// Requested semantic effect (lossless, preserve_bytes, exact timing, HDR, etc.) cannot be satisfied.
    UnsupportedEffect,
    /// Source color/pixel interpretation is unknown and cannot be assumed.
    UnknownInterpretation,
    /// Arbitrary ICC/CMYK transform required, but no eligible CMS backend is bound or `StrictNoIcc` is active.
    MissingCms,
    /// Operation policy or strict-bound admission refused the route.
    PolicyRefusal,
    /// A numeric, pixel, byte, or storage resource ceiling was exceeded.
    ResourceLimit,
    /// Cooperative cancellation or deadline expired.
    Stopped(StopReason),
    /// Input bitstream or container is structurally malformed.
    InvalidData,
    /// Closed-loop [`Coding::Verify`] exhausted its attempt budget without meeting the requested metric target.
    TargetUnmet,
    /// Verified output failed postcondition or structural readback checks.
    CorruptOutput,
    /// Underlying registered backend returned an error.
    BackendFailure,
    /// Transport I/O error.
    Io,
}

/// Why a candidate was filtered or skipped during selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RejectReason {
    /// Denied by `Routing` allow/deny/require rule.
    Denied,
    /// Backend lacks a required hard capability.
    MissingCapability,
    /// Candidate cannot satisfy admitted resource bounds (including unknown native workspace under strict mode).
    Limits,
    /// Candidate conflicts with requested semantics (e.g. `PreserveBytes`, `Lossless`, `ExactTiming`).
    Semantics,
    /// Candidate failed reversible preparation before authorization.
    Unbuildable,
    /// Candidate lacks required qualification evidence.
    Unproven,
}

/// Execution route family chosen by the planner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RouteKind {
    /// Byte-exact passthrough (`KeepBytes`).
    Passthrough,
    /// Direct encoded-domain transform (e.g. JPEG ↔ JXL reconstruction).
    Encoded,
    /// Streaming row-based pixel route.
    Rows,
    /// Full-raster pixel route.
    Raster,
    /// Timed animation frame route.
    Animation,
    /// Packet/frame media container pipeline (MP4/WebM/IVF).
    Media,
}

/// Structured selection trace event (never copies raw profiles, paths, or source bytes).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SelectionEvent {
    /// Recognized an image format during probe.
    DetectedImage(ImageFormat),
    /// Recognized a media container during probe.
    #[cfg(feature = "media")]
    DetectedContainer(ContainerId),
    /// Candidate rejected during filtering or reversible preparation.
    Rejected {
        /// Provider ID if associated with a specific registration.
        id: Option<RegistrationId>,
        /// Classification of why the candidate was rejected.
        reason: RejectReason,
    },
    /// Candidate selected for execution.
    Selected {
        /// Selected provider registration ID.
        id: RegistrationId,
        /// Route family chosen.
        route: RouteKind,
    },
    /// Explicit `AssumeSrgb` fallback applied to unspecified input.
    AssumedSrgb,
    /// CMS backend bound for an ICC conversion.
    CmsUsed {
        /// CMS backend identifier.
        id: String,
        /// CMS backend revision.
        revision: String,
    },
    /// Scalar metric score measured during [`Coding::Verify`].
    MetricMeasured {
        /// Requested metric.
        metric: Metric,
        /// Target scalar threshold.
        target: f32,
        /// Measured scalar score.
        observed: f32,
        /// Attempt ordinal (`1..=max_attempts`).
        attempt: u32,
    },
    /// Best-effort admission of an unknown native workspace bound (recorded only under explicit best-effort policy).
    UnknownWorkspaceAdmitted {
        /// Component whose native workspace upper bound is unknown.
        component: String,
    },
}

/// Bounded selection trace attached to [`Completion`] and [`EngineError`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectionTrace {
    events: Vec<SelectionEvent>,
    limit: usize,
    omitted: u64,
}

impl SelectionTrace {
    /// Hard upper bound on retained trace events per run.
    pub const MAX_CAP: usize = 256;

    /// Construct a trace collector capped at `min(limit, MAX_CAP)`.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            events: Vec::new(),
            limit: limit.min(Self::MAX_CAP),
            omitted: 0,
        }
    }

    /// Record an event, incrementing `omitted` once the cap is reached.
    pub fn push(&mut self, event: SelectionEvent) {
        if self.events.len() < self.limit {
            self.events.push(event);
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }

    /// Recorded events.
    #[must_use]
    pub fn events(&self) -> &[SelectionEvent] {
        &self.events
    }

    /// Number of events omitted due to the trace cap.
    #[must_use]
    pub const fn omitted(&self) -> u64 {
        self.omitted
    }
}

/// Gain-map effect summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GainMapEffect {
    /// Input had no gain map.
    #[default]
    Absent,
    /// Gain map preserved alongside base.
    Preserved,
    /// Gain map applied to reconstruct a single rendition.
    Reconstructed,
    /// Fresh gain map synthesized from HDR + SDR renditions.
    Synthesized,
    /// Existing gain map explicitly dropped.
    Dropped,
}

/// Observable execution report (`RouteReport` / `Effects`) returned on every operation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RouteReport {
    /// Selected route family.
    pub route: Option<RouteKind>,
    /// Emitted image format, if any.
    pub output_format: Option<ImageFormat>,
    /// Emitted media container, if any.
    #[cfg(feature = "media")]
    pub output_container: Option<ContainerId>,
    /// Emitted video codec, if any.
    #[cfg(feature = "media")]
    pub output_video_codec: Option<MediaCodecId>,
    /// Selected decoder registration ID.
    pub decoder: Option<RegistrationId>,
    /// Selected encoder or transform registration ID.
    pub encoder: Option<RegistrationId>,
    /// Selected demuxer registration ID.
    pub demuxer: Option<RegistrationId>,
    /// Selected muxer registration ID.
    pub muxer: Option<RegistrationId>,
    /// Active CMS provider ID if ICC conversion ran.
    pub cms_id: Option<String>,
    /// Whether primary presentation was explicitly selected from a sequence.
    pub selected_primary: bool,
    /// Whether HDR tone mapping executed.
    pub tone_mapped: bool,
    /// Gain-map disposition.
    pub gain_map: GainMapEffect,
    /// Requested metric when `Coding::Verify` ran.
    pub verified_metric: Option<Metric>,
    /// Effective scorer revision when `Coding::Verify` ran.
    pub verified_revision: Option<String>,
    /// Requested scalar metric target when `Coding::Verify` ran.
    pub verified_target: Option<f32>,
    /// Final measured scalar metric score when `Coding::Verify` ran (f32 compatibility field).
    pub verified_score: Option<f32>,
    /// Final measured native `f64` scalar metric score when `Coding::Verify` ran.
    pub verified_score_f64: Option<f64>,
    /// Selected calibrated quality parameter when `Coding::Verify` converged.
    pub verified_quality: Option<f32>,
    /// Number of verify attempts executed.
    pub verify_attempts: u32,
    /// Requested metric when one-shot `Coding::AimFor` ran.
    pub aim_metric: Option<Metric>,
    /// Requested target threshold when one-shot `Coding::AimFor` ran.
    pub aim_target: Option<f32>,
    /// Calibration heuristic revision when `Coding::AimFor` ran.
    pub aim_revision: Option<String>,
    /// Heuristic estimated quality selected when `Coding::AimFor` ran (unverified).
    pub aim_estimated_quality: Option<f32>,
    /// Components admitted with unknown native workspace bounds (non-empty ONLY under explicit best-effort policy).
    pub unknown_workspace_components: Vec<String>,
    /// Structured advisory warnings (e.g. unsupported advisory codec options).
    pub warnings: Vec<String>,
}

/// Error returned by [`Engine`] operations.
///
/// Boxed internally (`size_of::<EngineError>() == size_of::<usize>()`) to keep
/// `EngineResult<T>` small on constrained thread stacks (e.g. 256 KiB IIS workers).
/// Preserves stage, matchable failure classification, committed external bytes,
/// bounded reports/traces, and optional typed `source()` cause without leaking
/// source bytes, ICC profiles, or file paths.
pub struct EngineError(Box<EngineErrorInner>);

struct EngineErrorInner {
    stage: Stage,
    failure: Failure,
    message: String,
    committed_bytes: u64,
    decisions: OperationReport,
    report: RouteReport,
    trace: Option<SelectionTrace>,
    cause: Option<Box<dyn core::error::Error + Send + Sync>>,
}

impl EngineError {
    /// Create a structured error without a wrapped backend cause.
    #[must_use]
    pub fn simple(stage: Stage, failure: Failure, message: impl Into<String>) -> Self {
        Self(Box::new(EngineErrorInner {
            stage,
            failure,
            message: message.into(),
            committed_bytes: 0,
            decisions: OperationReport::default(),
            report: RouteReport::default(),
            trace: None,
            cause: None,
        }))
    }

    /// Attach a typed underlying cause.
    #[must_use]
    pub fn with_cause(mut self, cause: impl core::error::Error + Send + Sync + 'static) -> Self {
        self.0.cause = Some(Box::new(cause));
        self
    }

    /// Attach an already-boxed typed cause.
    #[must_use]
    pub fn with_boxed_cause(mut self, cause: Box<dyn core::error::Error + Send + Sync>) -> Self {
        self.0.cause = Some(cause);
        self
    }

    /// Attach committed external bytes count.
    #[must_use]
    pub fn with_committed_bytes(mut self, bytes: u64) -> Self {
        self.0.committed_bytes = bytes;
        self
    }

    /// Attach route report and optional trace.
    #[must_use]
    pub fn with_diagnostics(mut self, report: RouteReport, trace: Option<SelectionTrace>) -> Self {
        self.0.report = report;
        self.0.trace = trace;
        self
    }

    /// Failing pipeline stage.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.0.stage
    }

    /// Matchable failure classification.
    #[must_use]
    pub fn failure(&self) -> &Failure {
        &self.0.failure
    }

    /// Human-readable bounded error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// Bytes already irreversibly committed to an external incremental writer.
    #[must_use]
    pub fn committed_bytes(&self) -> u64 {
        self.0.committed_bytes
    }

    /// Authorization decisions recorded prior to failure.
    #[must_use]
    pub fn decisions(&self) -> &OperationReport {
        &self.0.decisions
    }

    /// Route report gathered prior to failure.
    #[must_use]
    pub fn route_report(&self) -> &RouteReport {
        &self.0.report
    }

    /// Selection trace if tracing was enabled.
    #[must_use]
    pub fn trace(&self) -> Option<&SelectionTrace> {
        self.0.trace.as_ref()
    }
}

impl fmt::Debug for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineError")
            .field("stage", &self.0.stage)
            .field("failure", &self.0.failure)
            .field("message", &self.0.message)
            .field("cause", &self.0.cause)
            .field("committed_bytes", &self.0.committed_bytes)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} ({:?}): {}",
            self.0.stage, self.0.failure, self.0.message
        )?;
        if let Some(ref c) = self.0.cause {
            write!(f, " (cause: {c})")?;
        }
        Ok(())
    }
}

impl core::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        self.0
            .cause
            .as_deref()
            .map(|e| e as &(dyn core::error::Error + 'static))
    }
}

impl From<RoutingError> for EngineError {
    fn from(e: RoutingError) -> Self {
        let msg = e.to_string();
        Self::simple(Stage::Admit, Failure::PolicyRefusal, msg).with_cause(e)
    }
}

/// Standard traced result type for [`Engine`] operations.
pub type EngineResult<T> = core::result::Result<T, At<EngineError>>;

/// Preparation error distinguishing reversible capability refusal (which returns
/// the untouched input/output lease) from terminal failures.
pub enum PrepareError<L> {
    /// Candidate cannot satisfy requested capability; untouched lease returned for fallback.
    Capability {
        /// Traced capability refusal cause.
        cause: At<EngineError>,
        /// Untouched caller lease returned verbatim.
        lease: L,
    },
    /// Terminal failure (malformed input, limit exceeded, stop, policy conflict, or I/O).
    Terminal(At<EngineError>),
}

impl<L> fmt::Debug for PrepareError<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capability { cause, .. } => {
                f.debug_struct("Capability").field("cause", cause).finish()
            }
            Self::Terminal(cause) => f.debug_tuple("Terminal").field(cause).finish(),
        }
    }
}

impl<L> PrepareError<L> {
    /// Convert any preparation error into the underlying traced [`EngineError`].
    #[must_use]
    pub fn into_error(self) -> At<EngineError> {
        match self {
            Self::Capability { cause, .. } | Self::Terminal(cause) => cause,
        }
    }
}

/// Result alias for [`Engine::prepare_decode`] / [`Engine::prepare_encode`] / [`Engine::prepare_transcode`].
pub type PrepareResult<T, L> = core::result::Result<T, PrepareError<L>>;

/// Successful operation output paired with its route report and optional trace.
#[derive(Clone, Debug)]
pub struct Completion<T> {
    /// Operation output value.
    pub output: T,
    /// Authorization report (boxed to keep return values compact on 256 KiB stacks).
    pub decisions: Box<OperationReport>,
    /// Observed route, selected providers, and effects.
    pub report: RouteReport,
    /// Optional bounded selection trace.
    pub trace: Option<SelectionTrace>,
}

/// Encoded image or media byte payload owned by [`Engine`].
#[derive(Clone, Debug)]
pub struct Encoded {
    bytes: Vec<u8>,
    format: Option<ImageFormat>,
    #[cfg(feature = "media")]
    container: Option<ContainerId>,
}

impl Encoded {
    /// Wrap encoded image bytes and their format.
    #[must_use]
    pub fn image(bytes: Vec<u8>, format: ImageFormat) -> Self {
        Self {
            bytes,
            format: Some(format),
            #[cfg(feature = "media")]
            container: None,
        }
    }

    /// Wrap encoded media container bytes.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn container(bytes: Vec<u8>, container: ContainerId) -> Self {
        Self {
            bytes,
            format: None,
            container: Some(container),
        }
    }

    /// Borrow encoded bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume and return owned byte buffer.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }

    /// Image format if this payload is a still or animated image.
    #[must_use]
    pub const fn format(&self) -> Option<ImageFormat> {
        self.format
    }

    /// Container identity if this payload is a video/media container.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn container_id(&self) -> Option<&ContainerId> {
        self.container.as_ref()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Per-operation Run binding & Engine
// ═══════════════════════════════════════════════════════════════════════════

/// Policy for handling providers whose native workspace upper bound is unknown.
///
/// Strict enforcement is the default (Decision 4). Unknown native workspaces
/// are admitted ONLY when a host explicitly enables [`WorkspacePolicy::BestEffort`],
/// and every unknown component is recorded in [`RouteReport::unknown_workspace_components`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WorkspacePolicy {
    /// Require proven upper bounds or isolated enforcement; refuse unknown native workspaces.
    #[default]
    Strict,
    /// Explicitly allow providers with unknown native workspace bounds while reporting them.
    BestEffort,
}

/// Per-operation execution binding (resources, policy, tightened routing, frozen color services).
#[derive(Clone)]
pub struct Run<'a> {
    pulse: SharedPulse,
    limits: Option<ResourceLimits>,
    storage: Option<StorageBudget>,
    routing: Option<&'a Routing>,
    policy: PolicySet,
    color_services: Option<ColorServices>,
    workspace_policy: WorkspacePolicy,
    max_trace_events: usize,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Run<'a> {
    /// Create a default run binding: inherits engine CMS/limits/routing, strict workspace bounds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pulse: SharedPulse::default(),
            limits: None,
            storage: None,
            routing: None,
            policy: PolicySet::new(),
            color_services: None,
            workspace_policy: WorkspacePolicy::Strict,
            max_trace_events: 0,
        }
    }

    /// Attach a cooperative cancellation/progress pulse.
    #[must_use]
    pub fn with_pulse(mut self, pulse: SharedPulse) -> Self {
        self.pulse = pulse;
        self
    }

    /// Tighten numeric resource limits (intersected with engine ceilings).
    #[must_use]
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Attach an explicit shared [`StorageBudget`] (must not exceed engine storage ceiling).
    #[must_use]
    pub fn with_storage(mut self, budget: StorageBudget) -> Self {
        self.storage = Some(budget);
        self
    }

    /// Tighten routing permissions and preferences under the engine's ceiling.
    #[must_use]
    pub const fn with_routing(mut self, routing: &'a Routing) -> Self {
        self.routing = Some(routing);
        self
    }

    /// Attach an authorization [`PolicySet`].
    #[must_use]
    pub const fn with_policy_set(mut self, policy: PolicySet) -> Self {
        self.policy = policy;
        self
    }

    /// Override CMS / tone-mapper services for this run (`Inherit`, `StrictNoIcc`, or `Use(binding)`).
    #[must_use]
    pub fn with_color_services(mut self, services: ColorServices) -> Self {
        self.color_services = Some(services);
        self
    }

    /// Explicitly opt into [`WorkspacePolicy::BestEffort`] for providers with unknown native
    /// workspace upper bounds. Every admitted unknown component is recorded in [`RouteReport`].
    #[must_use]
    pub const fn with_workspace_policy(mut self, policy: WorkspacePolicy) -> Self {
        self.workspace_policy = policy;
        self
    }

    /// Enable bounded selection tracing (capped at [`SelectionTrace::MAX_CAP`] = 256).
    #[must_use]
    pub fn with_trace(mut self, max_events: NonZeroU32) -> Self {
        self.max_trace_events = (max_events.get() as usize).min(SelectionTrace::MAX_CAP);
        self
    }

    /// Effective workspace bound enforcement policy.
    #[must_use]
    pub const fn workspace_policy(&self) -> WorkspacePolicy {
        self.workspace_policy
    }

    /// Borrow the cooperative stop/progress pulse bound to this run.
    #[must_use]
    pub const fn pulse(&self) -> &SharedPulse {
        &self.pulse
    }
}

/// Frozen effective execution state resolved from [`Engine`] + [`Run`].
pub struct EffectiveRun {
    /// Merged routing rules.
    pub routing: Routing,
    /// Shared resource budget, intersected limits, and pulse.
    pub resources: JobResources,
    /// Frozen color services (never `CmsChoice::Inherit`).
    pub color_services: ColorServices,
    /// Workspace enforcement policy.
    pub workspace_policy: WorkspacePolicy,
    /// Operation policy set.
    pub policy: PolicySet,
    /// Cumulative known native workspace bytes admitted across all live components in this run.
    pub admitted_workspace_bytes: u64,
    /// Optional bounded selection trace collector.
    pub trace: Option<SelectionTrace>,
}

impl EffectiveRun {
    /// Record an event if tracing is enabled.
    pub fn trace_event(&mut self, event: SelectionEvent) {
        if let Some(t) = &mut self.trace {
            t.push(event);
        }
    }

    /// Check cooperative cancellation.
    pub fn check_stop(&self, stage: Stage) -> EngineResult<()> {
        self.resources.check_stop().map_err(|reason| {
            at!(EngineError::simple(
                stage,
                Failure::Stopped(reason),
                "operation cancelled or deadline exceeded",
            ))
        })
    }

    /// Admit a provider workspace bound against [`WorkspacePolicy`], [`PolicySet`], and
    /// cumulative live native memory caps (`admitted_workspace_bytes`).
    pub fn admit_workspace(
        &mut self,
        component: &str,
        bound: NativeBound<u64>,
        report: &mut RouteReport,
    ) -> EngineResult<()> {
        match bound {
            NativeBound::Exact(bytes) | NativeBound::UpperBound(bytes) => {
                let cumulative = self
                    .admitted_workspace_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| {
                        at!(EngineError::simple(
                            Stage::Admit,
                            Failure::ResourceLimit,
                            alloc::format!("{component} cumulative native workspace overflow"),
                        ))
                    })?;
                if let Some(max_mem) = self.resources.limits().max_memory_bytes
                    && cumulative > max_mem
                {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::ResourceLimit,
                        alloc::format!(
                            "{component} cumulative native workspace ({cumulative} B) exceeds limit ({max_mem} B)"
                        ),
                    )));
                }
                if self.policy.check_workspace(Some(cumulative)).is_err() {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::PolicyRefusal,
                        alloc::format!(
                            "{component} cumulative native workspace ({cumulative} B) refused by PolicySet"
                        ),
                    )));
                }
                self.admitted_workspace_bytes = cumulative;
                Ok(())
            }
            NativeBound::Unknown => {
                if self.policy.check_workspace(None).is_err() {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::PolicyRefusal,
                        alloc::format!("{component} unknown native workspace refused by PolicySet"),
                    )));
                }
                match self.workspace_policy {
                    WorkspacePolicy::Strict => Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::PolicyRefusal,
                        alloc::format!(
                            "{component} has unknown native workspace bound (refused under Strict workspace policy)"
                        ),
                    ))),
                    WorkspacePolicy::BestEffort => {
                        report
                            .unknown_workspace_components
                            .push(component.to_string());
                        self.trace_event(SelectionEvent::UnknownWorkspaceAdmitted {
                            component: component.to_string(),
                        });
                        Ok(())
                    }
                }
            }
            _ => Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                alloc::format!("{component} reported unrecognized native bound variant"),
            ))),
        }
    }
}

/// Registered score-only metric scorer paired with its finite attempt cap.
#[derive(Clone)]
struct RegisteredScorer {
    scorer: Arc<dyn MetricScorer>,
    max_attempts: NonZeroU32,
}

/// Borrowed descriptor for one registered score-only [`MetricScorer`] on an [`Engine`].
#[derive(Clone, Copy)]
pub struct RegisteredMetricInfo<'a> {
    /// Registered metric enum variant.
    pub metric: Metric,
    /// Bound score-only scorer implementation.
    pub scorer: &'a Arc<dyn MetricScorer>,
    /// Per-job attempt cap enforced for [`Coding::Verify`].
    pub max_attempts: NonZeroU32,
}

/// Immutable codec engine holding one [`CodecSet`], routing ceiling, resource ceilings,
/// default [`ColorServices`], and optional score-only [`MetricScorer`] bindings.
#[derive(Clone)]
pub struct Engine {
    engine_id: usize,
    codecs: CodecSet,
    routing: Routing,
    limits: ResourceLimits,
    storage_max_bytes: usize,
    storage_max_objects: usize,
    color_services: ColorServices,
    scorers: BTreeMap<Metric, RegisteredScorer>,
}

/// Default conservative engine limits suitable for untrusted inputs.
#[must_use]
pub fn default_engine_limits() -> ResourceLimits {
    let mut lim = ResourceLimits::for_untrusted_input();
    lim.max_width = Some(8_192);
    lim.max_height = Some(8_192);
    lim.max_pixels = Some(32_000_000);
    lim.max_input_bytes = Some(32 * 1024 * 1024);
    lim.max_output_bytes = Some(32 * 1024 * 1024);
    lim.max_memory_bytes = Some(256 * 1024 * 1024);
    lim.max_frames = Some(512);
    lim
}

/// Ranked candidate offer for decoding an image format, immutably bound to its originating
/// [`Engine`], provider identity, and native workspace bound.
#[derive(Clone)]
pub struct DecodeOffer {
    engine_id: usize,
    id: RegistrationId,
    revision: &'static str,
    format: ImageFormat,
    config: Arc<dyn DynDecoderConfig>,
    workspace: NativeBound<u64>,
}

impl DecodeOffer {
    /// Registered provider ID.
    #[must_use]
    pub const fn id(&self) -> &RegistrationId {
        &self.id
    }

    /// Provider revision string.
    #[must_use]
    pub const fn revision(&self) -> &'static str {
        self.revision
    }

    /// Format handled by this offer.
    #[must_use]
    pub const fn format(&self) -> ImageFormat {
        self.format
    }

    /// Native workspace bound reported by this decoder (`Unknown` unless proven).
    #[must_use]
    pub const fn workspace(&self) -> NativeBound<u64> {
        self.workspace
    }
}

impl fmt::Debug for DecodeOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodeOffer")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("format", &self.format)
            .field("workspace", &self.workspace)
            .finish()
    }
}

/// Ranked candidate offer for encoding an image format, immutably bound to its provider identity
/// and native workspace bound.
#[derive(Clone)]
pub struct EncodeOffer {
    id: RegistrationId,
    revision: &'static str,
    format: ImageFormat,
    factory: Arc<dyn zencodec::ImageEncodeFactory>,
    workspace: NativeBound<u64>,
}

impl EncodeOffer {
    /// Registered provider ID.
    #[must_use]
    pub const fn id(&self) -> &RegistrationId {
        &self.id
    }

    /// Provider revision string.
    #[must_use]
    pub const fn revision(&self) -> &'static str {
        self.revision
    }

    /// Target image format.
    #[must_use]
    pub const fn format(&self) -> ImageFormat {
        self.format
    }

    /// Native workspace bound reported by the adapter (`Unknown` unless proven).
    #[must_use]
    pub const fn workspace(&self) -> NativeBound<u64> {
        self.workspace
    }
}

impl fmt::Debug for EncodeOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodeOffer")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("format", &self.format)
            .field("workspace", &self.workspace)
            .finish()
    }
}

/// Probed input lease holding the original source bytes and boxed probed facts.
pub struct ProbedInput<'io> {
    bytes: &'io [u8],
    inspection: Box<Inspection>,
}

impl<'io> ProbedInput<'io> {
    /// Borrow the underlying input bytes.
    #[must_use]
    pub const fn bytes(&self) -> &'io [u8] {
        self.bytes
    }

    /// Borrow probed inspection facts.
    #[must_use]
    pub fn inspection(&self) -> &Inspection {
        &self.inspection
    }
}

/// Prepared decode session bound to one exact selected offer and untouched input lease.
pub struct PreparedDecode<'io> {
    offer: DecodeOffer,
    input: ProbedInput<'io>,
    needs: DecodeNeeds,
    effective: EffectiveRun,
    report: RouteReport,
}

impl<'io> PreparedDecode<'io> {
    /// Selected provider ID.
    #[must_use]
    pub fn provider_id(&self) -> &RegistrationId {
        &self.offer.id
    }

    /// Execute the prepared decode and return pixels + route report.
    pub fn run(mut self) -> EngineResult<Completion<DecodeOutput>> {
        self.effective.check_stop(Stage::Execute)?;
        let mut job = self.offer.config.dyn_job();
        job.set_limits(*self.effective.resources.limits());
        job.set_pulse(self.effective.resources.pulse().clone());
        let hint = match self.needs.orientation {
            DecodeOrientation::Apply => zencodec::OrientationHint::Correct,
            DecodeOrientation::KeepStored | DecodeOrientation::Ignore => {
                zencodec::OrientationHint::Preserve
            }
        };
        job.set_orientation(hint);

        let preferred: Vec<PixelFormat> = self.needs.output_format.into_iter().collect();
        let decoder = job
            .into_decoder(Cow::Borrowed(self.input.bytes), &preferred)
            .map_err(|e| {
                classify_backend_error(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "decoder construction failed",
                    e,
                    &self.effective,
                    Some(&self.report),
                )
            })?;
        self.effective.check_stop(Stage::Execute)?;
        let decoded = decoder.decode().map_err(|e| {
            classify_backend_error(
                Stage::Execute,
                Failure::BackendFailure,
                "decode execution failed",
                e,
                &self.effective,
                Some(&self.report),
            )
        })?;
        self.effective.check_stop(Stage::Execute)?;

        self.report.route = Some(RouteKind::Raster);
        self.report.decoder = Some(self.offer.id.clone());
        Ok(Completion {
            output: decoded,
            decisions: Box::new(OperationReport::default()),
            report: self.report,
            trace: self.effective.trace,
        })
    }
}

impl Engine {
    /// Construct an immutable [`Engine`] from a registration-only [`CodecSet`].
    ///
    /// Rejects any [`CodecSet`] carrying pre-stamped operation defaults (`limits`,
    /// `pulse`, `decode_policy`, or `encode_policy`) so execution defaults always
    /// originate from one authoritative composition root.
    pub fn new(codecs: CodecSet) -> EngineResult<Self> {
        if codecs.has_operation_defaults() {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "Engine::new requires a registration-only CodecSet without pre-stamped operation defaults",
            )));
        }
        Ok(Self {
            engine_id: NEXT_ENGINE_ID.fetch_add(1, Ordering::Relaxed),
            codecs,
            routing: Routing::unrestricted(),
            limits: default_engine_limits(),
            storage_max_bytes: 256 * 1024 * 1024,
            storage_max_objects: 1_024,
            color_services: ColorServices::default(),
            scorers: BTreeMap::new(),
        })
    }

    /// Borrow the underlying immutable [`CodecSet`].
    #[must_use]
    pub const fn codecs(&self) -> &CodecSet {
        &self.codecs
    }

    /// Borrow the engine's ceiling [`Routing`].
    #[must_use]
    pub const fn routing(&self) -> &Routing {
        &self.routing
    }

    /// Borrow the engine's ceiling [`ResourceLimits`].
    #[must_use]
    pub const fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Borrow the engine's default [`ColorServices`].
    #[must_use]
    pub const fn color_services(&self) -> &ColorServices {
        &self.color_services
    }

    /// Iterate over all registered score-only [`MetricScorer`] bindings in deterministic [`Metric`] order.
    pub fn registered_metric_scorers(&self) -> impl Iterator<Item = RegisteredMetricInfo<'_>> {
        self.scorers
            .iter()
            .map(|(&metric, reg)| RegisteredMetricInfo {
                metric,
                scorer: &reg.scorer,
                max_attempts: reg.max_attempts,
            })
    }

    /// Look up the registered score-only [`MetricScorer`] binding for `metric`.
    #[must_use]
    pub fn metric_scorer(&self, metric: Metric) -> Option<RegisteredMetricInfo<'_>> {
        self.scorers.get(&metric).map(|reg| RegisteredMetricInfo {
            metric,
            scorer: &reg.scorer,
            max_attempts: reg.max_attempts,
        })
    }

    /// Set the engine's trusted ceiling [`Routing`].
    #[must_use]
    pub fn with_routing(mut self, ceiling: Routing) -> Self {
        self.routing = ceiling;
        self
    }

    /// Set the engine's ceiling [`ResourceLimits`].
    pub fn with_limits(mut self, limits: ResourceLimits) -> EngineResult<Self> {
        self.limits = limits;
        Ok(self)
    }

    /// Set the engine's managed storage budget ceiling.
    pub fn with_storage_limits(
        mut self,
        max_bytes: usize,
        max_objects: usize,
    ) -> EngineResult<Self> {
        if max_bytes == 0 || max_objects == 0 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "storage ceiling bytes and objects must be non-zero",
            )));
        }
        self.storage_max_bytes = max_bytes;
        self.storage_max_objects = max_objects;
        Ok(self)
    }

    /// Bind default [`ColorServices`] (CMS backend and/or tone mapper).
    #[must_use]
    pub fn with_color_services(mut self, services: ColorServices) -> Self {
        self.color_services = services;
        self
    }

    /// Register a score-only scalar [`MetricScorer`] for [`Coding::Verify`] with a finite attempt cap.
    ///
    /// Rejects duplicate bindings for the same [`Metric`].
    pub fn with_metric_scorer(
        mut self,
        scorer: Arc<dyn MetricScorer>,
        max_attempts: NonZeroU32,
    ) -> EngineResult<Self> {
        let metric = scorer.metric();
        if self.scorers.contains_key(&metric) {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "duplicate MetricScorer registration for the same Metric",
            )));
        }
        self.scorers.insert(
            metric,
            RegisteredScorer {
                scorer,
                max_attempts,
            },
        );
        Ok(self)
    }

    /// Resolve and freeze an [`EffectiveRun`] by intersecting `run` under `self`'s ceilings.
    pub fn resolve_run(&self, run: &Run<'_>) -> EngineResult<EffectiveRun> {
        let routing = match run.routing {
            Some(child) => self
                .routing
                .merge(child)
                .map_err(|e| at!(EngineError::from(e)))?,
            None => self.routing.clone(),
        };

        let limits = match &run.limits {
            Some(req) => intersect_limits(&self.limits, req),
            None => self.limits,
        };

        let storage = match &run.storage {
            Some(s) => {
                if s.max_bytes() > self.storage_max_bytes
                    || s.max_objects() > self.storage_max_objects
                {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::ResourceLimit,
                        "caller StorageBudget exceeds Engine storage ceiling",
                    )));
                }
                s.clone()
            }
            None => StorageBudget::new(self.storage_max_bytes, self.storage_max_objects),
        };

        let resources = JobResources::new(limits, storage, run.pulse.clone());

        let color_services = match &run.color_services {
            Some(req) => req
                .resolve_against(&self.color_services)
                .freeze_or_strict(None),
            None => self.color_services.freeze_or_strict(None),
        };
        if let CmsChoice::Use(binding) = color_services.cms_choice()
            && let Ok(rid) = RegistrationId::new(binding.id())
            && !routing.permits_provider(&RuleKey::Cms, &rid)
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                "configured CMS backend is denied by Routing",
            )));
        }

        let trace = if run.max_trace_events > 0 {
            Some(SelectionTrace::with_limit(run.max_trace_events))
        } else {
            None
        };

        Ok(EffectiveRun {
            routing,
            resources,
            color_services,
            workspace_policy: run.workspace_policy,
            policy: run.policy,
            admitted_workspace_bytes: 0,
            trace,
        })
    }

    /// Probe an input byte slice once under the engine's resource limits and registered detectors.
    pub fn probe<'io>(&self, bytes: &'io [u8], run: &Run<'_>) -> EngineResult<ProbedInput<'io>> {
        let mut eff = self.resolve_run(run)?;
        eff.check_stop(Stage::Inspect)?;
        if let Some(max_in) = eff.resources.limits().max_input_bytes
            && (bytes.len() as u64) > max_in
        {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::ResourceLimit,
                "input byte length exceeds max_input_bytes",
            )));
        }

        // 1. Check registered media containers first when `media` is enabled, distinguishing
        //    actual video MP4/WebM/IVF from still AVIF/HEIC (BMFF disambiguation).
        #[cfg(feature = "media")]
        {
            let media_probe = crate::media::probe::probe_container_bytes(
                &self.codecs,
                bytes,
                &zencodec_media::registry::ProbeLimits::default(),
            )?;
            if let Some((container, probed_by)) = media_probe {
                // Distinguish AVIF/HEIC still containers from MP4 video tracks:
                // if image detection identifies Avif/Heic and container is Mp4, check ftyp brand!
                let is_still_bmff = container == ContainerId::Mp4
                    && matches!(
                        self.codecs.detect(bytes),
                        Some(ImageFormat::Avif | ImageFormat::Heic)
                    );
                if !is_still_bmff {
                    eff.trace_event(SelectionEvent::DetectedContainer(container.clone()));
                    let env = MediaEnvelope {
                        kind: MediaKind::Video,
                        image_format: None,
                        container: Some(container),
                        width: 0,
                        height: 0,
                        has_alpha: false,
                        frame_count: None,
                        orientation: Orientation::Identity,
                        has_gain_map: false,
                        has_non_video_tracks: false,
                    };
                    return Ok(ProbedInput {
                        bytes,
                        inspection: Box::new(Inspection {
                            envelope: env,
                            image_info: None,
                            probed_by: Some(probed_by),
                        }),
                    });
                }
            }
        }

        // 2. Detect image format via registered CodecSet magic/detectors.
        let format = self.codecs.detect(bytes).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Inspect,
                Failure::UnrecognizedFormat,
                "input bytes did not match any recognized image or container format",
            ))
        })?;
        eff.trace_event(SelectionEvent::DetectedImage(format));

        if !eff.routing.permits_input_format(format) {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::DeniedFormat(format),
                "detected input format is denied by Routing",
            )));
        }

        // 3. Probe header using the top-ranked eligible decoder registration for `format`.
        let key = RuleKey::ImageDecode(format);
        let ranked = eff.routing.rank_candidates(
            &key,
            self.codecs
                .factories::<ImageDecodeRole>(&format)
                .map(|entry| (entry.id.clone(), entry)),
        );

        let has_named_decoders = self
            .codecs
            .factories::<ImageDecodeRole>(&format)
            .next()
            .is_some();
        let (info, probed_by) = if let Some((id, entry)) = ranked.first() {
            let mut job = entry.factory.dyn_job();
            job.set_limits(*eff.resources.limits());
            job.set_pulse(eff.resources.pulse().clone());
            let info = job.probe(bytes).map_err(|e| {
                classify_backend_error(
                    Stage::Inspect,
                    Failure::InvalidData,
                    "decoder header probe failed",
                    e,
                    &eff,
                    None,
                )
            })?;
            (info, Some(id.clone()))
        } else if !has_named_decoders
            && eff.routing.rule(&key).is_none()
            && let Some(dec) = self.codecs.decoder_for(format)
        {
            // Anonymous fallback is permitted ONLY when no named decoders exist AND no
            // routing rule/pin constrains this format.
            let mut job = dec.dyn_job();
            job.set_limits(*eff.resources.limits());
            job.set_pulse(eff.resources.pulse().clone());
            let info = job.probe(bytes).map_err(|e| {
                classify_backend_error(
                    Stage::Inspect,
                    Failure::InvalidData,
                    "decoder header probe failed",
                    e,
                    &eff,
                    None,
                )
            })?;
            (info, None)
        } else {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::NoBackend(format),
                "no eligible registered decoder available for detected image format",
            )));
        };

        // Check probed dimensions against effective limits before returning.
        check_dimensions(&info, eff.resources.limits())?;

        Ok(ProbedInput {
            bytes,
            inspection: Box::new(Inspection {
                envelope: MediaEnvelope::from_image_info(&info),
                image_info: Some(info),
                probed_by,
            }),
        })
    }

    /// Inspect an input byte slice and return its [`Inspection`] and route report.
    pub fn inspect(
        &self,
        request: &Inspect,
        bytes: &[u8],
        run: &Run<'_>,
    ) -> EngineResult<Completion<Inspection>> {
        let probed = self.probe(bytes, run)?;
        if let Some(expected) = request.expected_format
            && probed.inspection.envelope.image_format != Some(expected)
        {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidData,
                "detected format did not match Inspect::expected_format",
            )));
        }
        let report = RouteReport {
            decoder: probed.inspection.probed_by.clone(),
            ..Default::default()
        };
        Ok(Completion {
            output: *probed.inspection,
            decisions: Box::new(OperationReport::default()),
            report,
            trace: None,
        })
    }

    /// Compute deterministically ranked decode offers for a probed image input.
    ///
    /// Checks `DecodeNeeds::select_primary` against animated inputs upfront; workspace bounds
    /// and provider authorization are enforced again at [`Self::prepare_decode`].
    pub fn decode_offers(
        &self,
        src: &ProbedInput<'_>,
        need: &DecodeNeeds,
        run: &Run<'_>,
    ) -> EngineResult<Vec<DecodeOffer>> {
        let eff = self.resolve_run(run)?;
        let format = src.inspection.envelope.image_format.ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "decode_offers applies to image inputs; use media::route for video containers",
            ))
        })?;
        if src.inspection.envelope.kind == MediaKind::Animation && !need.select_primary {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "animated input requires sequence decode or explicit primary selection",
            )));
        }
        if !eff.routing.permits_input_format(format) {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::DeniedFormat(format),
                "input format denied by Routing",
            )));
        }
        let key = RuleKey::ImageDecode(format);
        let ranked = eff.routing.rank_candidates(
            &key,
            self.codecs
                .factories::<ImageDecodeRole>(&format)
                .map(|entry| (entry.id.clone(), entry)),
        );
        let env = &src.inspection.envelope;
        let dec_fmt = decode_pixel_format_from_info(
            src.inspection.image_info.as_ref(),
            need.output_format.as_ref(),
            env.has_alpha,
        );
        let offers: Vec<DecodeOffer> = ranked
            .into_iter()
            .map(|(id, entry)| {
                let workspace = qualified_still_decode_workspace(
                    entry.factory.as_ref(),
                    format,
                    env.width,
                    env.height,
                    &dec_fmt,
                );
                DecodeOffer {
                    engine_id: self.engine_id,
                    id,
                    revision: entry.revision,
                    format,
                    config: Arc::clone(&entry.factory),
                    workspace,
                }
            })
            .collect();
        if offers.is_empty() {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::NoBackend(format),
                "no eligible decoder backend for format",
            )));
        }
        Ok(offers)
    }

    /// Compute deterministically ranked encode offers for an [`OutputIntent`] and source envelope.
    pub fn encode_offers(
        &self,
        intent: &OutputIntent,
        media: &MediaEnvelope,
        run: &Run<'_>,
    ) -> EngineResult<Vec<EncodeOffer>> {
        intent.validate()?;
        let eff = self.resolve_run(run)?;
        let candidate_formats: Vec<ImageFormat> = match &intent.target {
            TargetFormat::Exact(fmt) => vec![*fmt],
            TargetFormat::Keep => {
                let src_fmt = media.image_format.ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Admit,
                        Failure::InvalidRequest,
                        "TargetFormat::Keep requires a known source image format",
                    ))
                })?;
                vec![src_fmt]
            }
            TargetFormat::Auto(prefs) => prefs.formats().to_vec(),
            #[cfg(feature = "media")]
            TargetFormat::Video { .. } => {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "video targets are routed through zencodecs::media::route",
                )));
            }
        };

        let mut offers = Vec::new();
        for format in candidate_formats {
            if !eff.routing.permits_output_format(format) {
                if matches!(intent.target, TargetFormat::Exact(_) | TargetFormat::Keep) {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::DeniedFormat(format),
                        "target output format is denied by Routing",
                    )));
                }
                continue;
            }
            let key = RuleKey::ImageEncode(format);
            let ranked = eff.routing.rank_candidates(
                &key,
                self.codecs
                    .factories::<ImageEncodeRole>(&format)
                    .map(|entry| (entry.id.clone(), entry)),
            );
            let offer_fmt = if media.has_alpha {
                PixelFormat::RGBA8_SRGB
            } else {
                PixelFormat::RGB8_SRGB
            };
            let offer_fidelity = fidelity_for_coding(&intent.coding);
            for (id, entry) in ranked {
                let workspace = qualified_still_encode_workspace(
                    entry.factory.config(),
                    format,
                    media.width,
                    media.height,
                    &offer_fmt,
                    offer_fidelity,
                );
                offers.push(EncodeOffer {
                    id,
                    revision: entry.revision,
                    format,
                    factory: Arc::clone(&entry.factory),
                    workspace,
                });
            }
        }

        if offers.is_empty() {
            let err_fmt = match &intent.target {
                TargetFormat::Exact(f) => *f,
                TargetFormat::Keep => media.image_format.unwrap_or(ImageFormat::Png),
                TargetFormat::Auto(p) => p.formats()[0],
                #[cfg(feature = "media")]
                TargetFormat::Video { .. } => unreachable!(),
            };
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::NoBackend(err_fmt),
                "no eligible registered encoder matches target format and Routing",
            )));
        }

        // Cap candidate fanout to 64 (matching zencodec PolicySet bitmask bound).
        if offers.len() > 64 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                "candidate encode offer fanout exceeds 64",
            )));
        }
        Ok(offers)
    }

    /// Prepare a decode session from a ranked offer while retaining the untouched lease on capability refusal.
    ///
    /// Revalidates engine identity, input format, active routing ceiling, registry membership,
    /// and workspace admission before constructing a [`PreparedDecode`].
    pub fn prepare_decode<'io>(
        &self,
        offer: DecodeOffer,
        src: ProbedInput<'io>,
        needs: DecodeNeeds,
        run: &Run<'_>,
    ) -> PrepareResult<PreparedDecode<'io>, ProbedInput<'io>> {
        let mut effective = match self.resolve_run(run) {
            Ok(e) => e,
            Err(err) => return Err(PrepareError::Terminal(err)),
        };
        if let Err(err) = effective.check_stop(Stage::Prepare) {
            return Err(PrepareError::Terminal(err));
        }

        // Revalidate offer provenance and active routing ceiling so offers from another Engine
        // or looser Run can never bypass a stricter Run's routing/policy.
        if offer.engine_id != self.engine_id {
            return Err(PrepareError::Terminal(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "DecodeOffer belongs to a different Engine instance",
            ))));
        }
        if src.inspection.envelope.image_format != Some(offer.format) {
            return Err(PrepareError::Terminal(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "DecodeOffer format does not match probed input format",
            ))));
        }
        if !effective.routing.permits_input_format(offer.format) {
            return Err(PrepareError::Terminal(at!(EngineError::simple(
                Stage::Admit,
                Failure::DeniedFormat(offer.format),
                "input format is denied by active Routing",
            ))));
        }
        let decode_key = RuleKey::ImageDecode(offer.format);
        if !effective.routing.permits_provider(&decode_key, &offer.id) {
            return Err(PrepareError::Terminal(at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                "DecodeOffer provider is denied or excluded by active Routing",
            ))));
        }
        let Some(entry) = self
            .codecs
            .factories::<ImageDecodeRole>(&offer.format)
            .find(|e| e.id == offer.id)
        else {
            return Err(PrepareError::Terminal(at!(EngineError::simple(
                Stage::Admit,
                Failure::NoBackend(offer.format),
                "DecodeOffer provider is not registered in this Engine",
            ))));
        };
        // Rebind config and canonical workspace directly from the validated registry entry so
        // offer identity, decoder config, and workspace bound can never diverge.
        let env = &src.inspection.envelope;
        let dec_fmt = decode_pixel_format_from_info(
            src.inspection.image_info.as_ref(),
            needs.output_format.as_ref(),
            env.has_alpha,
        );
        let workspace = qualified_still_decode_workspace(
            entry.factory.as_ref(),
            offer.format,
            env.width,
            env.height,
            &dec_fmt,
        );
        let validated_offer = DecodeOffer {
            engine_id: self.engine_id,
            id: entry.id.clone(),
            revision: entry.revision,
            format: offer.format,
            config: Arc::clone(&entry.factory),
            workspace,
        };

        // Refuse multi-frame sequence if caller requested still-only without primary selection.
        if src.inspection.envelope.kind == MediaKind::Animation && !needs.select_primary {
            let err = at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "animated input requires sequence decode or explicit primary selection",
            ));
            return Err(PrepareError::Capability {
                cause: err,
                lease: src,
            });
        }

        let mut report = RouteReport {
            decoder: Some(validated_offer.id.clone()),
            ..Default::default()
        };
        // Admit canonical decoder workspace bound against WorkspacePolicy + PolicySet.
        if let Err(err) = effective.admit_workspace(
            validated_offer.id.as_str(),
            validated_offer.workspace,
            &mut report,
        ) {
            return Err(PrepareError::Capability {
                cause: err,
                lease: src,
            });
        }
        effective.trace_event(SelectionEvent::Selected {
            id: validated_offer.id.clone(),
            route: RouteKind::Raster,
        });

        Ok(PreparedDecode {
            offer: validated_offer,
            input: src,
            needs,
            effective,
            report,
        })
    }

    /// Decode an image byte slice through the unified `probe -> decode_offers -> prepare_decode -> run` path.
    pub fn decode(
        &self,
        needs: &DecodeNeeds,
        bytes: &[u8],
        run: &Run<'_>,
    ) -> EngineResult<Completion<DecodeOutput>> {
        let mut probed = self.probe(bytes, run)?;
        let offers = self.decode_offers(&probed, needs, run)?;
        let mut last_err = None;
        for offer in offers {
            match self.prepare_decode(offer, probed, needs.clone(), run) {
                Ok(prepared) => return prepared.run(),
                Err(PrepareError::Capability { cause, lease }) => {
                    last_err = Some(cause);
                    probed = lease;
                }
                Err(PrepareError::Terminal(err)) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "no decoder offer satisfied decode requirements",
            ))
        }))
    }

    /// Encode a pixel buffer view using the unified offer, color, metadata, and verify pipeline.
    pub fn encode_pixels(
        &self,
        intent: &OutputIntent,
        pixels: PixelView<'_>,
        metadata: &Metadata,
        source_format: Option<ImageFormat>,
        run: &Run<'_>,
    ) -> EngineResult<Completion<Encoded>> {
        intent.validate()?;
        let mut eff = self.resolve_run(run)?;
        eff.check_stop(Stage::Prepare)?;

        let envelope = MediaEnvelope {
            kind: MediaKind::Still,
            image_format: source_format,
            #[cfg(feature = "media")]
            container: None,
            width: pixels.width(),
            height: pixels.height(),
            has_alpha: pixels.format().color.alpha() != AlphaMode::None,
            frame_count: Some(1),
            orientation: metadata.orientation,
            has_gain_map: false,
            has_non_video_tracks: false,
        };

        // Enforce Decision 2: `PreserveBytes` refuses any required rewrite (including EXIF orientation rewrite).
        if intent.metadata.requires_byte_exact_payloads()
            && intent.orientation == OrientationMode::Bake
            && metadata.orientation != Orientation::Identity
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "MetadataPolicy::PreserveBytes forbids EXIF orientation rewrite when baking non-identity orientation",
            )));
        }

        // Enforce original still-SDR eligibility for `Coding::Verify` BEFORE any orientation
        // bake, color conversion, or alpha flattening can erase HDR/unknown/invalid evidence!
        if matches!(intent.coding, Coding::Verify(_)) {
            validate_sdr_verify_view(&pixels, "original source")?;
            validate_sdr_verify_metadata(metadata, "original source")?;
        }

        let offers = self.encode_offers(intent, &envelope, run)?;
        let mut report = RouteReport::default();
        let mut live_pre_bytes: u64 = 0;

        let check_pre_alloc = |live: &mut u64, add: u64, eff: &EffectiveRun| -> EngineResult<()> {
            let next = live.saturating_add(add);
            if let Some(max_mem) = eff.resources.limits().max_memory_bytes
                && next > max_mem
            {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::ResourceLimit,
                    "pre-encode pixel transform allocation exceeds max_memory_bytes",
                )));
            }
            *live = next;
            Ok(())
        };

        // Bake D4 orientation onto pixels when `OrientationMode::Bake` is requested on
        // non-identity metadata orientation (and reconcile metadata orientation to Identity).
        let baked_buf: Option<PixelBuf>;
        let mut effective_meta = metadata.clone();
        let oriented_view: PixelView<'_> = if intent.orientation == OrientationMode::Bake
            && metadata.orientation != Orientation::Identity
        {
            let (dw, dh) = metadata
                .orientation
                .output_dimensions(pixels.width(), pixels.height());
            let bake_bytes = u64::from(dw)
                .saturating_mul(u64::from(dh))
                .saturating_mul(pixels.format().layout.bytes_per_pixel() as u64);
            check_pre_alloc(&mut live_pre_bytes, bake_bytes, &eff)?;
            let b = bake_orientation_pixels(pixels, metadata.orientation, &eff)?;
            effective_meta.orientation = Orientation::Identity;
            baked_buf = Some(b);
            baked_buf.as_ref().expect("just set").as_view()
        } else {
            pixels
        };

        let meta_has_color_signal = |m: &Metadata| -> bool {
            m.icc_profile.as_ref().is_some_and(|b| !b.is_empty())
                || m.cicp.is_some()
                || m.content_light_level.is_some()
                || m.mastering_display.is_some()
                || m.diffuse_white.is_some()
        };

        // Resolve authoritative source format (honoring embedded ICC when authoritative) BEFORE
        // any format-equality shortcut, color conversion, alpha flattening, or PreserveBytes check!
        let oriented_eff_fmt =
            effective_format_with_metadata(oriented_view.format(), &effective_meta)?;

        // Prepare color conversion if explicit target format requested.
        let converted_buf: Option<PixelBuf>;
        let (colored_view, colored_eff_fmt): (PixelView<'_>, PixelFormat) = match &intent.color {
            ColorTarget::Preserve => (oriented_view, oriented_eff_fmt),
            ColorTarget::ConvertTo(target_fmt) if *target_fmt == oriented_eff_fmt => {
                (oriented_view, oriented_eff_fmt)
            }
            ColorTarget::ConvertTo(target_fmt) => {
                if intent.metadata.requires_byte_exact_payloads()
                    && (meta_has_color_signal(&effective_meta)
                        || *oriented_view.format() != oriented_eff_fmt)
                {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::UnsupportedEffect,
                        "MetadataPolicy::PreserveBytes forbids pixel color conversion that alters or drops embedded color profile metadata",
                    )));
                }
                let conv_bytes = u64::from(oriented_view.width())
                    .saturating_mul(u64::from(oriented_view.height()))
                    .saturating_mul(target_fmt.layout.bytes_per_pixel() as u64);
                check_pre_alloc(&mut live_pre_bytes, conv_bytes, &eff)?;
                let buf = convert_pixel_view_with_src_format(
                    oriented_view,
                    &oriented_eff_fmt,
                    target_fmt.clone(),
                    &mut eff,
                    &mut report,
                )?;
                reconcile_converted_metadata(&mut effective_meta, target_fmt);
                converted_buf = Some(buf);
                (
                    converted_buf.as_ref().expect("just set").as_view(),
                    target_fmt.clone(),
                )
            }
        };

        // Enforce explicit AlphaPolicy (`RequireOpaque` or `FlattenSrgb` in linear light).
        let flattened_buf: Option<PixelBuf>;
        let encode_view: PixelView<'_> = match intent.alpha {
            AlphaPolicy::Preserve => colored_view,
            AlphaPolicy::RequireOpaque => {
                if pixel_view_has_translucent_alpha(&colored_view, &eff)? {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::UnsupportedEffect,
                        "AlphaPolicy::RequireOpaque refused input with non-opaque alpha",
                    )));
                }
                colored_view
            }
            AlphaPolicy::FlattenSrgb(matte) => {
                if matches!(
                    colored_eff_fmt.color.alpha(),
                    AlphaMode::Straight | AlphaMode::Premultiplied
                ) {
                    if intent.metadata.requires_byte_exact_payloads()
                        && (meta_has_color_signal(&effective_meta)
                            || *colored_view.format() != colored_eff_fmt)
                    {
                        return Err(at!(EngineError::simple(
                            Stage::Admit,
                            Failure::UnsupportedEffect,
                            "MetadataPolicy::PreserveBytes forbids AlphaPolicy::FlattenSrgb conversion that alters or drops embedded color profile metadata",
                        )));
                    }
                    let rgba_scratch = if colored_eff_fmt == PixelFormat::RGBA8_SRGB {
                        0u64
                    } else {
                        u64::from(colored_view.width())
                            .saturating_mul(u64::from(colored_view.height()))
                            .saturating_mul(4)
                    };
                    let flat_bytes = u64::from(colored_view.width())
                        .saturating_mul(u64::from(colored_view.height()))
                        .saturating_mul(3);
                    check_pre_alloc(
                        &mut live_pre_bytes,
                        rgba_scratch.saturating_add(flat_bytes),
                        &eff,
                    )?;
                    let b = flatten_alpha_onto_srgb_matte(
                        colored_view,
                        &colored_eff_fmt,
                        matte,
                        &mut eff,
                        &mut report,
                    )?;
                    live_pre_bytes = live_pre_bytes.saturating_sub(rgba_scratch);
                    reconcile_converted_metadata(&mut effective_meta, &PixelFormat::RGB8_SRGB);
                    flattened_buf = Some(b);
                    flattened_buf.as_ref().expect("just set").as_view()
                } else {
                    colored_view
                }
            }
        };

        // Dispatch `Coding::Verify(MetricTarget)` to the bounded score-only search loop.
        if let Coding::Verify(ref target) = intent.coding {
            return self.encode_pixels_verified(
                intent,
                target,
                encode_view,
                &effective_meta,
                live_pre_bytes,
                &offers,
                eff,
                report,
            );
        }

        // Narrow remaining memory budget for ordinary still encode by subtracting live
        // pre-encode buffers, and re-evaluate workspace bounds for the actual emitted raster
        // and resolved fidelity configuration.
        let orig_limits = *eff.resources.limits();
        let orig_storage = eff.resources.storage().clone();
        let orig_pulse = eff.resources.pulse().clone();
        if let Some(max_mem) = orig_limits.max_memory_bytes {
            let Some(rem) = max_mem.checked_sub(live_pre_bytes) else {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::ResourceLimit,
                    "pre-encode pixel working set exceeds max_memory_bytes",
                )));
            };
            let mut narrowed = orig_limits;
            narrowed.max_memory_bytes = Some(rem);
            eff.resources = JobResources::new(narrowed, orig_storage, orig_pulse);
        }

        // Select top admissible offer respecting `WorkspacePolicy` for the actual emitted raster.
        let resolved_fidelity = fidelity_for_coding(&intent.coding);
        let mut selected = None;
        for mut offer in offers {
            offer.workspace = qualified_still_encode_workspace(
                offer.factory.config(),
                offer.format,
                encode_view.width(),
                encode_view.height(),
                encode_view.format(),
                resolved_fidelity,
            );
            let mut probe_report = report.clone();
            match eff.admit_workspace(offer.id.as_str(), offer.workspace, &mut probe_report) {
                Ok(()) => {
                    report = probe_report;
                    selected = Some(offer);
                    break;
                }
                Err(e) => {
                    eff.trace_event(SelectionEvent::Rejected {
                        id: Some(offer.id.clone()),
                        reason: RejectReason::Limits,
                    });
                    if !eff
                        .routing
                        .allows_fallback(&RuleKey::ImageEncode(offer.format))
                    {
                        return Err(e);
                    }
                }
            }
        }
        let offer = selected.ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                "no encode offer satisfied workspace policy and limits",
            ))
        })?;

        let out = self.run_single_encode(
            &offer,
            intent,
            encode_view.clone(),
            &effective_meta,
            None,
            &mut eff,
            &mut report,
        )?;

        self.verify_preserve_bytes_readback(
            intent,
            offer.format,
            &effective_meta,
            out.data(),
            &mut eff,
            &mut report,
        )?;
        eff.check_stop(Stage::Verify)?;

        report.route = Some(RouteKind::Raster);
        report.output_format = Some(offer.format);
        report.encoder = Some(offer.id.clone());
        eff.trace_event(SelectionEvent::Selected {
            id: offer.id,
            route: RouteKind::Raster,
        });

        Ok(Completion {
            output: Encoded::image(out.into_vec(), offer.format),
            decisions: Box::new(OperationReport::default()),
            report,
            trace: eff.trace,
        })
    }

    /// Transcode an image byte slice through the unified `probe -> route -> decode -> convert -> encode -> verify` pipeline.
    pub fn transcode(
        &self,
        intent: &OutputIntent,
        bytes: &[u8],
        run: &Run<'_>,
    ) -> EngineResult<Completion<Encoded>> {
        intent.validate()?;
        let mut eff = self.resolve_run(run)?;
        eff.check_stop(Stage::Inspect)?;

        let probed = self.probe(bytes, run)?;
        let src_format = probed.inspection.envelope.image_format.ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "video container transcode runs via zencodecs::media::pump",
            ))
        })?;

        // Handle `Coding::KeepBytes` passthrough with strict metadata & orientation checks.
        if matches!(intent.coding, Coding::KeepBytes) {
            let target_fmt = match &intent.target {
                TargetFormat::Exact(f) => *f,
                TargetFormat::Keep => src_format,
                TargetFormat::Auto(p) => {
                    if p.formats().contains(&src_format) {
                        src_format
                    } else {
                        return Err(at!(EngineError::simple(
                            Stage::Admit,
                            Failure::UnsupportedEffect,
                            "KeepBytes requires target format to match detected source format",
                        )));
                    }
                }
                #[cfg(feature = "media")]
                TargetFormat::Video { .. } => {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::UnsupportedEffect,
                        "KeepBytes cannot target a video container from an image input",
                    )));
                }
            };
            if target_fmt != src_format {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "KeepBytes requires identical source and target format",
                )));
            }
            if !eff.routing.permits_output_format(target_fmt) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::DeniedFormat(target_fmt),
                    "target format denied by Routing",
                )));
            }
            // Do not bypass metadata stripping (EXIF, XMP, ICC, CICP, HDR) or orientation bake on passthrough.
            let info = probed.inspection.image_info.as_ref();
            let strips_present_metadata = info.is_some_and(|i| {
                let has_exif_or_xmp =
                    i.embedded_metadata.exif.is_some() || i.embedded_metadata.xmp.is_some();
                let has_color_or_hdr = i.source_color.icc_profile.is_some()
                    || i.source_color.cicp.is_some()
                    || i.source_color.content_light_level.is_some()
                    || i.source_color.mastering_display.is_some();
                match intent.metadata {
                    MetadataPolicy::StripAll => {
                        has_exif_or_xmp
                            || has_color_or_hdr
                            || i.orientation != Orientation::Identity
                    }
                    MetadataPolicy::Web | MetadataPolicy::ColorAndRotation => has_exif_or_xmp,
                    MetadataPolicy::Preserve | MetadataPolicy::PreserveBytes => false,
                    MetadataPolicy::Custom(fields) => {
                        let orig = Metadata::from(i);
                        let filt = orig.filtered(&zencodec::MetadataPolicy::Custom(fields));
                        orig.exif != filt.exif
                            || orig.xmp != filt.xmp
                            || orig.icc_profile != filt.icc_profile
                            || orig.cicp != filt.cicp
                            || orig.content_light_level != filt.content_light_level
                            || orig.mastering_display != filt.mastering_display
                            || orig.orientation != filt.orientation
                    }
                }
            });
            if strips_present_metadata {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "KeepBytes conflicts with metadata stripping on metadata-bearing input",
                )));
            }
            if intent.orientation == OrientationMode::Bake
                && info.is_some_and(|i| i.orientation != Orientation::Identity)
            {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "KeepBytes conflicts with baking non-identity EXIF orientation",
                )));
            }
            let report = RouteReport {
                route: Some(RouteKind::Passthrough),
                output_format: Some(target_fmt),
                ..Default::default()
            };
            return Ok(Completion {
                output: Encoded::image(bytes.to_vec(), target_fmt),
                decisions: Box::new(OperationReport::default()),
                report,
                trace: eff.trace,
            });
        }

        // Check registered EncodedTransformRole for `ExactJpegRoundTrip`.
        if matches!(intent.coding, Coding::ExactJpegRoundTrip) {
            let target_fmt = match &intent.target {
                TargetFormat::Exact(f) => *f,
                _ => {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::InvalidRequest,
                        "ExactJpegRoundTrip requires TargetFormat::Exact(Jxl | Jpeg)",
                    )));
                }
            };
            if !eff.routing.permits_output_format(target_fmt) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::DeniedFormat(target_fmt),
                    "target format denied by Routing for ExactJpegRoundTrip",
                )));
            }
            let pair = (src_format, target_fmt);
            let key = RuleKey::EncodedTransform(src_format, target_fmt);
            let ranked = eff.routing.rank_candidates(
                &key,
                self.codecs
                    .factories::<EncodedTransformRole>(&pair)
                    .map(|entry| (entry.id.clone(), entry)),
            );
            let Some((id, entry)) = ranked.into_iter().next() else {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::NoBackend(target_fmt),
                    "no registered EncodedTransformRole backend for exact JPEG round-trip pair",
                )));
            };
            let mut report = RouteReport {
                route: Some(RouteKind::Encoded),
                output_format: Some(target_fmt),
                encoder: Some(id.clone()),
                ..Default::default()
            };
            eff.admit_workspace(id.as_str(), NativeBound::Unknown, &mut report)?;
            let encoded = entry
                .factory
                .transform_bytes(bytes, Some(eff.resources.pulse()))
                .map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "encoded transform execution failed",
                    )
                    .with_boxed_cause(e))
                })?;
            eff.trace_event(SelectionEvent::Selected {
                id,
                route: RouteKind::Encoded,
            });
            return Ok(Completion {
                output: Encoded::image(encoded, target_fmt),
                decisions: Box::new(OperationReport::default()),
                report,
                trace: eff.trace,
            });
        }

        // Standard pixel transcode: validate original still-SDR eligibility for Coding::Verify
        // BEFORE decoding or converting pixels so HDR/gain-map evidence cannot be lost!
        if matches!(intent.coding, Coding::Verify(_)) {
            if probed.inspection.envelope.has_gain_map {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "Coding::Verify refuses HDR gain-map inputs",
                )));
            }
            if let Some(info) = probed.inspection.image_info.as_ref() {
                validate_sdr_verify_source_color(&info.source_color, "probed source")?;
            }
        }

        // Standard pixel transcode: decode -> encode_pixels using exact same metadata & orientation.
        let decode_needs = DecodeNeeds {
            output_format: None,
            orientation: match intent.orientation {
                OrientationMode::Bake => DecodeOrientation::Apply,
                OrientationMode::PreserveStored => DecodeOrientation::KeepStored,
            },
            select_primary: matches!(intent.sequence, SequencePolicy::FirstPresentation),
            extract_gain_map: matches!(
                intent.gain_map,
                GainMapEncodePolicy::Preserve | GainMapEncodePolicy::Require
            ),
        };
        let decoded = self.decode(&decode_needs, bytes, run)?;
        let decoder_id = decoded.report.decoder.clone();
        let meta = decoded.output.metadata();
        let view = decoded.output.pixels();

        // Note: if PreserveBytes was requested AND orientation was non-identity with Bake,
        // `probed` preserves the original orientation fact before decode baked it!
        if intent.metadata.requires_byte_exact_payloads()
            && intent.orientation == OrientationMode::Bake
            && probed.inspection.envelope.orientation != Orientation::Identity
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "MetadataPolicy::PreserveBytes forbids EXIF orientation rewrite when baking non-identity orientation",
            )));
        }

        let mut encoded = self.encode_pixels(intent, view, &meta, Some(src_format), run)?;
        encoded.report.decoder = decoder_id;
        encoded.report.selected_primary =
            matches!(intent.sequence, SequencePolicy::FirstPresentation)
                && probed.inspection.envelope.kind != MediaKind::Still;
        Ok(encoded)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_single_encode(
        &self,
        offer: &EncodeOffer,
        intent: &OutputIntent,
        pixels: PixelView<'_>,
        metadata: &Metadata,
        quality_override: Option<f32>,
        eff: &mut EffectiveRun,
        report: &mut RouteReport,
    ) -> EngineResult<EncodeOutput> {
        eff.check_stop(Stage::Execute)?;
        let fidelity = match quality_override {
            Some(q) => Some(Fidelity::codec_quality(q)),
            None => match &intent.coding {
                Coding::Auto => None,
                Coding::CalibratedQuality(q) => Some(Fidelity::codec_quality(*q)),
                Coding::QualityProfile { level, dpr } => {
                    Some(Fidelity::codec_quality(level.calibrated_quality(*dpr)))
                }
                Coding::AimFor(t) => {
                    t.validate()?;
                    if let Some(ref expected_rev) = t.revision
                        && expected_rev != AIM_SEED_REVISION
                    {
                        return Err(at!(EngineError::simple(
                            Stage::Admit,
                            Failure::UnsupportedEffect,
                            "Coding::AimFor calibration revision does not match pinned target revision",
                        )));
                    }
                    let est_q = calibrated_aim_seed(t);
                    report.aim_metric = Some(t.metric);
                    report.aim_target = Some(t.value);
                    report.aim_revision = Some(AIM_SEED_REVISION.to_string());
                    report.aim_estimated_quality = Some(est_q);
                    Some(Fidelity::codec_quality(est_q))
                }
                Coding::Verify(t) => Some(Fidelity::codec_quality(calibrated_aim_seed(t))),
                Coding::Lossless => Some(Fidelity::Lossless),
                Coding::LosslessOrKeep => Some(Fidelity::codec_quality(
                    QualityLevel::Good.calibrated_quality(1.0),
                )),
                Coding::KeepBytes | Coding::ExactJpegRoundTrip => None,
            },
        };
        let mut job = match fidelity {
            Some(f) => offer.factory.config().try_fidelity_job(f).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::UnsupportedEffect,
                    "encoder refused requested fidelity",
                )
                .with_cause(e))
            })?,
            None => offer.factory.config().dyn_job(),
        };
        job.set_limits(*eff.resources.limits());
        job.set_pulse(eff.resources.pulse().clone());

        let filtered_meta = metadata.filtered(&intent.metadata.to_zencodec());
        job.set_unfiltered_metadata(filtered_meta);

        let encoder = job.into_encoder().map_err(|e| {
            classify_backend_error(
                Stage::Prepare,
                Failure::BackendFailure,
                "encoder instantiation failed",
                e,
                eff,
                Some(report),
            )
        })?;
        eff.check_stop(Stage::Execute)?;
        let encoded = encoder.encode(pixels).map_err(|e| {
            classify_backend_error(
                Stage::Execute,
                Failure::BackendFailure,
                "encode execution failed",
                e,
                eff,
                Some(report),
            )
        })?;
        eff.check_stop(Stage::Execute)?;

        if let Some(max_out) = eff.resources.limits().max_output_bytes
            && (encoded.data().len() as u64) > max_out
        {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::ResourceLimit,
                "encoded output exceeds max_output_bytes",
            )));
        }

        Ok(encoded)
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_pixels_verified(
        &self,
        intent: &OutputIntent,
        target: &MetricTarget,
        pixels: PixelView<'_>,
        metadata: &Metadata,
        live_pre_bytes: u64,
        offers: &[EncodeOffer],
        mut eff: EffectiveRun,
        mut report: RouteReport,
    ) -> EngineResult<Completion<Encoded>> {
        const Q_MIN: f32 = 5.0;
        const Q_MAX: f32 = 100.0;
        const Q_TOL: f32 = 0.5;

        target.validate()?;
        eff.check_stop(Stage::Admit)?;

        // 1. Validate emitted SDR interpretation, HDR view signaling, and metadata BEFORE any shortcut.
        validate_sdr_verify_view(&pixels, "emitted")?;
        validate_sdr_verify_metadata(metadata, "emitted")?;

        let reg = self.scorers.get(&target.metric).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "no registered MetricScorer for requested Metric",
            ))
        })?;
        if let Some(expected_rev) = &target.revision
            && reg.scorer.revision() != expected_rev
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "registered MetricScorer revision does not match pinned target revision",
            )));
        }

        // Compute upright reference dimensions upfront before any allocation or readback selection.
        let cmp_fmt = reg.scorer.comparison_format();
        let needs_upright_bake = intent.orientation == OrientationMode::PreserveStored
            && metadata.orientation != Orientation::Identity;
        let (ref_w, ref_h) = if needs_upright_bake {
            metadata
                .orientation
                .output_dimensions(pixels.width(), pixels.height())
        } else {
            (pixels.width(), pixels.height())
        };
        // Resolve authoritative reference format (honoring embedded ICC profile if authoritative).
        let ref_src_fmt = effective_format_with_metadata(pixels.format(), metadata)?;
        validate_sdr_verify_interpretation(&ref_src_fmt, "authoritative reference")?;

        // 2. Qualify still-SDR encoder across quality-resolved configurations, calibrated quality
        //    dial endpoints, AND exact bound workspace-admissible readback decoder BEFORE allocating.
        type SelectedVerifyRoute<'a> = (
            &'a EncodeOffer,
            NativeBound<u64>,
            RegistrationId,
            Arc<dyn DynDecoderConfig>,
            NativeBound<u64>,
        );
        let mut selected: Option<SelectedVerifyRoute<'_>> = None;
        let mut last_admit_err: Option<At<EngineError>> = None;

        // Readback decode expects `cmp_fmt` (or native emitted layout if wider); pick the wider
        // descriptor for upfront decode workspace admission so sample depth is never under-estimated.
        let upfront_dec_fmt =
            if pixels.format().layout.bytes_per_pixel() > cmp_fmt.layout.bytes_per_pixel() {
                pixels.format().clone()
            } else {
                cmp_fmt.clone()
            };

        for candidate in offers {
            let mut probe_report = report.clone();
            let mut probe_eff = EffectiveRun {
                routing: eff.routing.clone(),
                resources: eff.resources.clone(),
                color_services: eff.color_services.clone(),
                workspace_policy: eff.workspace_policy,
                policy: eff.policy,
                admitted_workspace_bytes: eff.admitted_workspace_bytes,
                trace: None,
            };
            // Qualify calibrated quality dial across endpoints AND midpoint AND take the worst-case
            // quality-resolved encoder workspace bound across those configurations.
            let mut dial_ok = true;
            let mut worst_enc_ws: Option<NativeBound<u64>> = None;
            for test_q in [Q_MIN, 50.0, Q_MAX] {
                if let Err(e) = resolve_effective_verify_setting(candidate.factory.config(), test_q)
                {
                    eff.trace_event(SelectionEvent::Rejected {
                        id: Some(candidate.id.clone()),
                        reason: RejectReason::MissingCapability,
                    });
                    last_admit_err = Some(e);
                    dial_ok = false;
                    break;
                }
                let q_ws = qualified_still_encode_workspace(
                    candidate.factory.config(),
                    candidate.format,
                    pixels.width(),
                    pixels.height(),
                    pixels.format(),
                    Some(Fidelity::codec_quality(test_q)),
                );
                worst_enc_ws = Some(match (worst_enc_ws, q_ws) {
                    (None, b) => b,
                    (Some(NativeBound::Unknown), _) | (_, NativeBound::Unknown) => {
                        NativeBound::Unknown
                    }
                    (Some(a), b) => {
                        let av = match a {
                            NativeBound::Exact(v) | NativeBound::UpperBound(v) => v,
                            _ => 0,
                        };
                        let bv = match b {
                            NativeBound::Exact(v) | NativeBound::UpperBound(v) => v,
                            _ => 0,
                        };
                        NativeBound::UpperBound(av.max(bv))
                    }
                });
            }
            if !dial_ok {
                if !eff
                    .routing
                    .allows_fallback(&RuleKey::ImageEncode(candidate.format))
                {
                    return Err(last_admit_err.unwrap());
                }
                continue;
            }
            let enc_ws = worst_enc_ws.unwrap_or(NativeBound::Unknown);
            if let Err(e) =
                probe_eff.admit_workspace(candidate.id.as_str(), enc_ws, &mut probe_report)
            {
                eff.trace_event(SelectionEvent::Rejected {
                    id: Some(candidate.id.clone()),
                    reason: RejectReason::Limits,
                });
                last_admit_err = Some(e);
                if !eff
                    .routing
                    .allows_fallback(&RuleKey::ImageEncode(candidate.format))
                {
                    return Err(last_admit_err.unwrap());
                }
                continue;
            }
            // Verify matching routed readback decoder exists, satisfies WorkspacePolicy,
            // and bind that exact decoder for every Verify readback probe + decode.
            if !eff.routing.permits_input_format(candidate.format) {
                last_admit_err = Some(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::PolicyRefusal,
                    "Coding::Verify readback format is denied by active Routing",
                )));
                if !eff
                    .routing
                    .allows_fallback(&RuleKey::ImageEncode(candidate.format))
                {
                    return Err(last_admit_err.unwrap());
                }
                continue;
            }
            let dec_key = RuleKey::ImageDecode(candidate.format);
            let dec_ranked = eff.routing.rank_candidates(
                &dec_key,
                self.codecs
                    .factories::<ImageDecodeRole>(&candidate.format)
                    .map(|entry| (entry.id.clone(), entry)),
            );
            let mut dec_bound: Option<(
                RegistrationId,
                Arc<dyn DynDecoderConfig>,
                NativeBound<u64>,
            )> = None;
            let mut dec_admit_err: Option<At<EngineError>> = None;
            for (dec_id, dec_entry) in dec_ranked {
                let pb = qualified_still_probe_bound(dec_entry.factory.as_ref(), 0);
                let db = qualified_still_decode_workspace(
                    dec_entry.factory.as_ref(),
                    candidate.format,
                    ref_w,
                    ref_h,
                    &upfront_dec_fmt,
                );
                let mut dec_probe_report = probe_report.clone();
                let mut dec_probe_eff = EffectiveRun {
                    routing: eff.routing.clone(),
                    resources: eff.resources.clone(),
                    color_services: eff.color_services.clone(),
                    workspace_policy: eff.workspace_policy,
                    policy: eff.policy,
                    admitted_workspace_bytes: probe_eff.admitted_workspace_bytes,
                    trace: None,
                };
                match dec_probe_eff
                    .admit_workspace(dec_id.as_str(), pb, &mut dec_probe_report)
                    .and_then(|()| {
                        dec_probe_eff.admit_workspace(dec_id.as_str(), db, &mut dec_probe_report)
                    }) {
                    Ok(()) => {
                        probe_report = dec_probe_report;
                        dec_bound = Some((dec_id, Arc::clone(&dec_entry.factory), db));
                        break;
                    }
                    Err(e) => {
                        dec_admit_err = Some(e);
                        if !eff.routing.allows_fallback(&dec_key) {
                            break;
                        }
                    }
                }
            }
            let Some((dec_id, dec_cfg, db)) = dec_bound else {
                eff.trace_event(SelectionEvent::Rejected {
                    id: Some(candidate.id.clone()),
                    reason: RejectReason::MissingCapability,
                });
                last_admit_err = Some(dec_admit_err.unwrap_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Admit,
                        Failure::UnsupportedEffect,
                        "Coding::Verify requires an eligible workspace-admissible readback decoder",
                    ))
                }));
                if !eff
                    .routing
                    .allows_fallback(&RuleKey::ImageEncode(candidate.format))
                {
                    return Err(last_admit_err.unwrap());
                }
                continue;
            };
            report = probe_report;
            eff.admitted_workspace_bytes = probe_eff.admitted_workspace_bytes;
            selected = Some((candidate, enc_ws, dec_id, dec_cfg, db));
            break;
        }
        let (offer, enc_ws_bound, readback_dec_id, readback_dec_cfg, readback_dec_bound) = selected
            .ok_or_else(|| {
                last_admit_err.unwrap_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Admit,
                        Failure::NoBackend(ImageFormat::Jpeg),
                        "no eligible encoder/decoder route satisfied Coding::Verify admission",
                    ))
                })
            })?;

        // Refuse opaque-only target formats when non-opaque alpha is present without FlattenSrgb.
        if matches!(intent.alpha, AlphaPolicy::Preserve)
            && matches!(offer.format, ImageFormat::Jpeg | ImageFormat::Bmp)
            && pixel_view_has_translucent_alpha(&pixels, &eff)?
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "opaque-only output format cannot preserve non-opaque alpha without AlphaPolicy::FlattenSrgb",
            )));
        }

        // 3. Admit combined peak memory BEFORE allocating upright reference, normalized reference,
        //    prepared scorer reference, or executing any encode/readback trial.
        let orig_limits = *eff.resources.limits();
        let orig_storage = eff.resources.storage().clone();
        let orig_pulse = eff.resources.pulse().clone();

        let upright_bytes = if needs_upright_bake {
            u64::from(ref_w)
                .saturating_mul(u64::from(ref_h))
                .saturating_mul(pixels.format().layout.bytes_per_pixel() as u64)
        } else {
            0
        };
        let cmp_bpp = cmp_fmt.layout.bytes_per_pixel() as u64;
        let cmp_raster_bytes = u64::from(ref_w)
            .saturating_mul(u64::from(ref_h))
            .saturating_mul(cmp_bpp);
        let ref_conv_bytes = if ref_src_fmt == cmp_fmt {
            0
        } else {
            cmp_raster_bytes
        };
        let scorer_scratch = reg.scorer.estimated_scratch_bytes(ref_w, ref_h);
        let enc_ws_bytes = match enc_ws_bound {
            NativeBound::Exact(b) | NativeBound::UpperBound(b) => b,
            _ => 0,
        };
        let readback_ws_bytes = match readback_dec_bound {
            NativeBound::Exact(b) | NativeBound::UpperBound(b) => b,
            _ => 0,
        };
        let live_ref_hold = live_pre_bytes
            .saturating_add(upright_bytes)
            .saturating_add(ref_conv_bytes);
        let trial_stage_peak = enc_ws_bytes.max(readback_ws_bytes.saturating_add(cmp_raster_bytes));
        let base_peak_bytes = live_ref_hold
            .saturating_add(scorer_scratch)
            .saturating_add(trial_stage_peak);
        if let Some(max_mem) = orig_limits.max_memory_bytes
            && base_peak_bytes > max_mem
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::ResourceLimit,
                alloc::format!(
                    "Coding::Verify combined peak memory ({base_peak_bytes} B) exceeds max_memory_bytes ({max_mem} B)"
                ),
            )));
        }

        let make_remaining_resources =
            |held_bytes: u64, cap_output: bool| -> EngineResult<JobResources> {
                let mut limits = orig_limits;
                if let Some(max_mem) = orig_limits.max_memory_bytes {
                    let Some(rem) = max_mem.checked_sub(held_bytes) else {
                        return Err(at!(EngineError::simple(
                            Stage::Verify,
                            Failure::ResourceLimit,
                            "Coding::Verify live working set exceeds max_memory_bytes",
                        )));
                    };
                    limits.max_memory_bytes = Some(rem);
                    if cap_output {
                        limits.max_output_bytes =
                            intersect_opt_u64(orig_limits.max_output_bytes, Some(rem));
                    }
                }
                Ok(JobResources::new(
                    limits,
                    orig_storage.clone(),
                    orig_pulse.clone(),
                ))
            };

        // Run upright preparation, warm scorer preparation, and bounded trial search inside an
        // explicit inner block so `prepared`, `ref_buf`, and `upright_buf` are unconditionally
        // released BEFORE the final `verify_preserve_bytes_readback` probe executes.
        let (accepted, score_f64, selected_q) = {
            // 4. Build upright reference view matching actual emitted presentation orientation
            //    (with cooperative cancellation polling inside the orientation loop).
            let upright_buf: Option<PixelBuf>;
            let upright_ref: PixelView<'_> = if needs_upright_bake {
                let b = bake_orientation_pixels(pixels.clone(), metadata.orientation, &eff)?;
                upright_buf = Some(b);
                upright_buf.as_ref().expect("just set").as_view()
            } else {
                pixels.clone()
            };

            // 5. Normalize reference raster once (honoring authoritative ICC if present) and prepare
            //    reusable metric reference once with remaining memory budget.
            let ref_buf: Option<PixelBuf>;
            let ref_view = if ref_src_fmt == cmp_fmt {
                upright_ref
            } else {
                let b = convert_pixel_view_with_src_format(
                    upright_ref,
                    &ref_src_fmt,
                    cmp_fmt.clone(),
                    &mut eff,
                    &mut report,
                )?;
                ref_buf = Some(b);
                ref_buf.as_ref().expect("just set").as_view()
            };
            eff.check_stop(Stage::Prepare)?;
            let prep_resources = make_remaining_resources(live_ref_hold, false)?;
            let mut prepared = reg
                .scorer
                .prepare_reference(ref_view.clone(), &prep_resources)?;

            // 6. Bracket-and-bisect bounded search over calibrated quality [5.0, 100.0],
            //    keyed by post-quantization effective encoder setting.
            let max_attempts = reg.max_attempts.get();
            let target_f64 = f64::from(target.value);
            let mut attempts: u32 = 0;
            let mut tested_keys: Vec<u32> = Vec::with_capacity(max_attempts as usize);
            let mut best_passing: Option<(EncodeOutput, f64, f32)> = None;
            let mut best_observed: Option<(f64, f32)> = None;

            report.verified_metric = Some(target.metric);
            report.verified_revision = Some(reg.scorer.revision().to_string());
            report.verified_target = Some(target.value);

            let mut run_trial = |req_q: f32,
                                 eff_q: f32,
                                 attempts: &mut u32,
                                 best_passing: &mut Option<(EncodeOutput, f64, f32)>,
                                 best_observed: &mut Option<(f64, f32)>,
                                 eff: &mut EffectiveRun,
                                 report: &mut RouteReport|
             -> EngineResult<bool> {
                eff.check_stop(Stage::Verify)?;
                *attempts = attempts.saturating_add(1);
                let attempt = *attempts;

                // Re-admit the exact trial quality-resolved encoder configuration through
                // `admit_workspace` so WorkspacePolicy (Strict) AND PolicySet both apply!
                let retained_bytes = best_passing
                    .as_ref()
                    .map_or(0u64, |(b, _, _)| b.data().len() as u64);
                let pre_enc_live = live_ref_hold
                    .saturating_add(scorer_scratch)
                    .saturating_add(retained_bytes);
                let trial_enc_bound = qualified_still_encode_workspace(
                    offer.factory.config(),
                    offer.format,
                    pixels.width(),
                    pixels.height(),
                    pixels.format(),
                    Some(Fidelity::codec_quality(req_q)),
                );
                let trial_enc_resources = make_remaining_resources(pre_enc_live, true)?;
                let mut trial_enc_check = EffectiveRun {
                    routing: eff.routing.clone(),
                    resources: trial_enc_resources.clone(),
                    color_services: eff.color_services.clone(),
                    workspace_policy: eff.workspace_policy,
                    policy: eff.policy,
                    admitted_workspace_bytes: 0,
                    trace: None,
                };
                trial_enc_check.admit_workspace(offer.id.as_str(), trial_enc_bound, report)?;

                eff.resources = trial_enc_resources;
                let encoded_res = self.run_single_encode(
                    offer,
                    intent,
                    pixels.clone(),
                    metadata,
                    Some(req_q),
                    eff,
                    report,
                );
                eff.resources =
                    JobResources::new(orig_limits, orig_storage.clone(), orig_pulse.clone());
                let encoded = encoded_res?;

                // Admit provider-specific header-probe workspace bound BEFORE calling `probe`.
                let trial_bytes = encoded.data().len() as u64;
                let pre_dec_live = pre_enc_live.saturating_add(trial_bytes);
                let dec_resources = make_remaining_resources(pre_dec_live, false)?;
                let probe_bound =
                    qualified_still_probe_bound(readback_dec_cfg.as_ref(), trial_bytes);
                let mut probe_check_eff = EffectiveRun {
                    routing: eff.routing.clone(),
                    resources: dec_resources.clone(),
                    color_services: eff.color_services.clone(),
                    workspace_policy: eff.workspace_policy,
                    policy: eff.policy,
                    admitted_workspace_bytes: 0,
                    trace: None,
                };
                probe_check_eff.admit_workspace(readback_dec_id.as_str(), probe_bound, report)?;

                // Probe via the exact admitted readback decoder BEFORE decoding.
                let mut probe_job = readback_dec_cfg.dyn_job();
                probe_job.set_limits(*dec_resources.limits());
                probe_job.set_pulse(dec_resources.pulse().clone());
                let probe_info = probe_job.probe(encoded.data()).map_err(|e| {
                    classify_backend_error(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "Coding::Verify readback header probe failed",
                        e,
                        eff,
                        Some(report),
                    )
                })?;
                eff.check_stop(Stage::Verify)?;

                if probe_info.supplements.gain_map {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::UnsupportedEffect,
                        "Coding::Verify readback output unexpectedly contains HDR gain map",
                    )));
                }
                if probe_info.is_animation() {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "Coding::Verify readback output unexpectedly probed as animation",
                    )));
                }
                let (probe_up_w, probe_up_h) = probe_info
                    .orientation
                    .output_dimensions(probe_info.width, probe_info.height);
                if probe_up_w != ref_w || probe_up_h != ref_h {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        alloc::format!(
                            "Coding::Verify readback probed upright dimensions {probe_up_w}x{probe_up_h} do not match reference {ref_w}x{ref_h}",
                        ),
                    )));
                }
                validate_sdr_verify_source_color(&probe_info.source_color, "readback candidate")?;

                // Re-admit full readback decode workspace using probed sample depth/channels +
                // requested output format before constructing the decoder.
                let probed_native_fmt =
                    decode_pixel_format_from_info(Some(&probe_info), None, probe_info.has_alpha);
                let probed_dec_fmt = if probed_native_fmt.layout.bytes_per_pixel()
                    > cmp_fmt.layout.bytes_per_pixel()
                {
                    probed_native_fmt
                } else {
                    cmp_fmt.clone()
                };
                let probed_dec_bound = qualified_still_decode_workspace(
                    readback_dec_cfg.as_ref(),
                    offer.format,
                    probe_info.width,
                    probe_info.height,
                    &probed_dec_fmt,
                );
                let mut dec_check_eff = EffectiveRun {
                    routing: eff.routing.clone(),
                    resources: dec_resources.clone(),
                    color_services: eff.color_services.clone(),
                    workspace_policy: eff.workspace_policy,
                    policy: eff.policy,
                    admitted_workspace_bytes: 0,
                    trace: None,
                };
                dec_check_eff.admit_workspace(
                    readback_dec_id.as_str(),
                    probed_dec_bound,
                    report,
                )?;

                // Decode candidate back to upright pixels through the exact admitted readback decoder.
                let mut dec_job = readback_dec_cfg.dyn_job();
                dec_job.set_limits(*dec_resources.limits());
                dec_job.set_pulse(dec_resources.pulse().clone());
                dec_job.set_orientation(zencodec::OrientationHint::Correct);
                let preferred = [cmp_fmt.clone()];
                let decoder = dec_job
                    .into_decoder(Cow::Borrowed(encoded.data()), &preferred)
                    .map_err(|e| {
                        classify_backend_error(
                            Stage::Verify,
                            Failure::BackendFailure,
                            "Coding::Verify readback decoder construction failed",
                            e,
                            eff,
                            Some(report),
                        )
                    })?;
                eff.check_stop(Stage::Verify)?;
                let decoded = decoder.decode().map_err(|e| {
                    classify_backend_error(
                        Stage::Verify,
                        Failure::BackendFailure,
                        "Coding::Verify readback decode execution failed",
                        e,
                        eff,
                        Some(report),
                    )
                })?;
                eff.check_stop(Stage::Verify)?;
                let cand_raw = decoded.pixels();

                // Verify upright dimensions, view HDR signaling, and SDR interpretation BEFORE shortcut.
                if cand_raw.width() != ref_w || cand_raw.height() != ref_h {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        alloc::format!(
                            "Coding::Verify readback upright dimensions {}x{} do not match reference {}x{}",
                            cand_raw.width(),
                            cand_raw.height(),
                            ref_w,
                            ref_h,
                        ),
                    )));
                }
                validate_sdr_verify_view(&cand_raw, "decoded candidate")?;
                let cand_src_fmt = effective_format_with_source_color(
                    cand_raw.format(),
                    &probe_info.source_color,
                )?;
                validate_sdr_verify_interpretation(&cand_src_fmt, "authoritative candidate")?;

                let cand_raw_bytes =
                    (cand_raw.row_bytes() as u64).saturating_mul(u64::from(cand_raw.height()));
                let cand_conv_bytes = if cand_src_fmt == cmp_fmt {
                    0
                } else {
                    cmp_raster_bytes
                };
                if let Some(max_mem) = orig_limits.max_memory_bytes
                    && pre_dec_live
                        .saturating_add(cand_raw_bytes)
                        .saturating_add(cand_conv_bytes)
                        > max_mem
                {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::ResourceLimit,
                        "Coding::Verify decoded candidate conversion exceeds max_memory_bytes",
                    )));
                }

                let cand_buf: Option<PixelBuf>;
                let cand_view = if cand_src_fmt == cmp_fmt {
                    cand_raw
                } else {
                    let b = convert_pixel_view_with_src_format(
                        cand_raw,
                        &cand_src_fmt,
                        cmp_fmt.clone(),
                        eff,
                        report,
                    )?;
                    cand_buf = Some(b);
                    cand_buf.as_ref().expect("just set").as_view()
                };

                let score_held = live_ref_hold
                    .saturating_add(retained_bytes)
                    .saturating_add(trial_bytes)
                    .saturating_add(cand_raw_bytes)
                    .saturating_add(cand_conv_bytes);
                let score_resources = make_remaining_resources(score_held, false)?;

                let observed = match prepared.as_mut() {
                    Some(p) => p.score_candidate_f64(cand_view, &score_resources)?,
                    None => reg
                        .scorer
                        .score_f64(ref_view.clone(), cand_view, &score_resources)?,
                };
                if !observed.is_finite() {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::BackendFailure,
                        "MetricScorer returned non-finite scalar score",
                    )));
                }

                eff.trace_event(SelectionEvent::MetricMeasured {
                    metric: target.metric,
                    target: target.value,
                    observed: observed as f32,
                    attempt,
                });
                report.verify_attempts = attempt;

                let better_obs = match best_observed {
                    None => true,
                    Some((prev_obs, _)) => match target.metric {
                        Metric::ButteraugliMax => observed < *prev_obs,
                        Metric::Ssimulacra2 | Metric::ZensimA | Metric::ZensimB => {
                            observed > *prev_obs
                        }
                    },
                };
                if better_obs {
                    *best_observed = Some((observed, eff_q));
                }

                let passed = target.metric.satisfies_f64(observed, target_f64);
                if passed {
                    let replace = match best_passing {
                        None => true,
                        Some((prev_enc, _, prev_q)) => {
                            eff_q < *prev_q
                                || ((eff_q - *prev_q).abs() < 1e-6
                                    && encoded.data().len() < prev_enc.data().len())
                        }
                    };
                    if replace {
                        *best_passing = Some((encoded, observed, eff_q));
                    }
                }
                Ok(passed)
            };

            let mut fail_lo: Option<(f32, u32)> = None;
            let mut pass_hi: Option<(f32, u32)> = None;

            // Probe 1: calibrated seed.
            let seed_q = calibrated_aim_seed(target).clamp(Q_MIN, Q_MAX);
            let (seed_key, seed_eff_q) =
                resolve_effective_verify_setting(offer.factory.config(), seed_q)?;
            tested_keys.push(seed_key);
            if run_trial(
                seed_q,
                seed_eff_q,
                &mut attempts,
                &mut best_passing,
                &mut best_observed,
                &mut eff,
                &mut report,
            )? {
                pass_hi = Some((seed_q, seed_key));
            } else {
                fail_lo = Some((seed_q, seed_key));
            }

            // Probe 2: endpoint bracketing while budget remains, deduplicated by effective setting.
            if attempts < max_attempts {
                if let Some((hi, _)) = pass_hi {
                    if (hi - Q_MIN) >= Q_TOL {
                        let (min_key, min_eff_q) =
                            resolve_effective_verify_setting(offer.factory.config(), Q_MIN)?;
                        if !tested_keys.contains(&min_key) {
                            tested_keys.push(min_key);
                            if run_trial(
                                Q_MIN,
                                min_eff_q,
                                &mut attempts,
                                &mut best_passing,
                                &mut best_observed,
                                &mut eff,
                                &mut report,
                            )? {
                                pass_hi = Some((Q_MIN, min_key));
                            } else {
                                fail_lo = Some((Q_MIN, min_key));
                            }
                        }
                    }
                } else if let Some((lo, _)) = fail_lo
                    && (Q_MAX - lo) >= Q_TOL
                {
                    let (max_key, max_eff_q) =
                        resolve_effective_verify_setting(offer.factory.config(), Q_MAX)?;
                    if !tested_keys.contains(&max_key) {
                        tested_keys.push(max_key);
                        if run_trial(
                            Q_MAX,
                            max_eff_q,
                            &mut attempts,
                            &mut best_passing,
                            &mut best_observed,
                            &mut eff,
                            &mut report,
                        )? {
                            pass_hi = Some((Q_MAX, max_key));
                        } else {
                            fail_lo = Some((Q_MAX, max_key));
                        }
                    }
                }
            }

            // Probes 3..=max_attempts: bisect observed [fail_lo, pass_hi] bracket without ever
            // spending a trial on an already-evaluated effective encoder setting.
            while attempts < max_attempts {
                let (Some((lo, lo_key)), Some((hi, hi_key))) = (fail_lo, pass_hi) else {
                    break;
                };
                if (hi - lo) < Q_TOL {
                    break;
                }
                let mid = (0.5 * (lo + hi)).clamp(Q_MIN, Q_MAX);
                let (mid_key, mid_eff_q) =
                    resolve_effective_verify_setting(offer.factory.config(), mid)?;
                if mid_key == lo_key {
                    fail_lo = Some((mid, lo_key));
                    continue;
                }
                if mid_key == hi_key {
                    pass_hi = Some((mid, hi_key));
                    continue;
                }
                if tested_keys.contains(&mid_key) {
                    break;
                }
                tested_keys.push(mid_key);
                if run_trial(
                    mid,
                    mid_eff_q,
                    &mut attempts,
                    &mut best_passing,
                    &mut best_observed,
                    &mut eff,
                    &mut report,
                )? {
                    pass_hi = Some((mid, mid_key));
                } else {
                    fail_lo = Some((mid, mid_key));
                }
            }

            // Check cancellation before committing/returning the selected candidate.
            eff.check_stop(Stage::Verify)?;

            best_passing.ok_or_else(|| {
                let obs_note = match best_observed {
                    Some((obs, q)) => alloc::format!(" (best observed {obs:.4} at q={q:.1})"),
                    None => alloc::string::String::new(),
                };
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::TargetUnmet,
                    alloc::format!(
                        "Coding::Verify exhausted {} attempt(s) without meeting {} target {}{}",
                        attempts,
                        target.metric.as_token(),
                        target.value,
                        obs_note,
                    ),
                )
                .with_diagnostics(report.clone(), eff.trace.clone()))
            })?
        };
        // `prepared`, `ref_buf`, and `upright_buf` are now explicitly dropped!

        let preserve_resources = make_remaining_resources(
            live_pre_bytes.saturating_add(accepted.data().len() as u64),
            false,
        )?;
        eff.resources = preserve_resources;
        self.verify_preserve_bytes_readback(
            intent,
            offer.format,
            metadata,
            accepted.data(),
            &mut eff,
            &mut report,
        )?;
        eff.check_stop(Stage::Verify)?;

        report.route = Some(RouteKind::Raster);
        report.output_format = Some(offer.format);
        report.encoder = Some(offer.id.clone());
        report.verified_score = Some(score_f64 as f32);
        report.verified_score_f64 = Some(score_f64);
        report.verified_quality = Some(selected_q);
        eff.trace_event(SelectionEvent::Selected {
            id: offer.id.clone(),
            route: RouteKind::Raster,
        });

        Ok(Completion {
            output: Encoded::image(accepted.into_vec(), offer.format),
            decisions: Box::new(OperationReport::default()),
            report,
            trace: eff.trace,
        })
    }

    /// Enforce Decision 2 postcondition shared across both ordinary and `Verify` still encode paths:
    /// selects a decoder with a proven header-probe bound through active `Routing`, binds its exact
    /// probe execution, and verifies every requested EXIF/XMP/ICC byte payload survived verbatim.
    #[allow(clippy::too_many_arguments)]
    fn verify_preserve_bytes_readback(
        &self,
        intent: &OutputIntent,
        format: ImageFormat,
        effective_meta: &Metadata,
        encoded_bytes: &[u8],
        eff: &mut EffectiveRun,
        report: &mut RouteReport,
    ) -> EngineResult<()> {
        if !intent.metadata.requires_byte_exact_payloads() {
            return Ok(());
        }
        let needs_verify = effective_meta.exif.as_ref().is_some_and(|b| !b.is_empty())
            || effective_meta.xmp.as_ref().is_some_and(|b| !b.is_empty())
            || effective_meta
                .icc_profile
                .as_ref()
                .is_some_and(|b| !b.is_empty());
        if !needs_verify {
            return Ok(());
        }
        eff.check_stop(Stage::Verify)?;
        if !eff.routing.permits_input_format(format) {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::PolicyRefusal,
                "MetadataPolicy::PreserveBytes readback format is denied by active Routing",
            )));
        }
        let key = RuleKey::ImageDecode(format);
        let ranked = eff.routing.rank_candidates(
            &key,
            self.codecs
                .factories::<ImageDecodeRole>(&format)
                .map(|entry| (entry.id.clone(), entry)),
        );
        let mut selected_dec = None;
        let mut last_err = None;
        for (dec_id, dec_entry) in ranked {
            let pb =
                qualified_still_probe_bound(dec_entry.factory.as_ref(), encoded_bytes.len() as u64);
            let mut probe_report = report.clone();
            match eff.admit_workspace(dec_id.as_str(), pb, &mut probe_report) {
                Ok(()) => {
                    *report = probe_report;
                    selected_dec = Some(Arc::clone(&dec_entry.factory));
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if !eff.routing.allows_fallback(&key) {
                        return Err(last_err.unwrap());
                    }
                }
            }
        }
        let dec_cfg = selected_dec.ok_or_else(|| {
            last_err.unwrap_or_else(|| {
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::UnsupportedEffect,
                    "MetadataPolicy::PreserveBytes requires an eligible workspace-admissible decoder to verify byte-exact metadata retention",
                ))
            })
        })?;
        let mut probe_job = dec_cfg.dyn_job();
        probe_job.set_limits(*eff.resources.limits());
        probe_job.set_pulse(eff.resources.pulse().clone());
        let readback = probe_job.probe(encoded_bytes).map_err(|e| {
            classify_backend_error(
                Stage::Verify,
                Failure::CorruptOutput,
                "encoded output failed MetadataPolicy::PreserveBytes readback probe",
                e,
                eff,
                Some(report),
            )
        })?;
        eff.check_stop(Stage::Verify)?;
        if let Some(expected) = effective_meta.exif.as_deref().filter(|b| !b.is_empty())
            && readback.embedded_metadata.exif.as_deref() != Some(expected)
        {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::UnsupportedEffect,
                "encoder did not preserve EXIF bytes verbatim under MetadataPolicy::PreserveBytes",
            )));
        }
        if let Some(expected) = effective_meta.xmp.as_deref().filter(|b| !b.is_empty())
            && readback.embedded_metadata.xmp.as_deref() != Some(expected)
        {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::UnsupportedEffect,
                "encoder did not preserve XMP bytes verbatim under MetadataPolicy::PreserveBytes",
            )));
        }
        if let Some(expected) = effective_meta
            .icc_profile
            .as_deref()
            .filter(|b| !b.is_empty())
            && readback.source_color.icc_profile.as_deref() != Some(expected)
        {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::UnsupportedEffect,
                "encoder did not preserve ICC profile bytes verbatim under MetadataPolicy::PreserveBytes",
            )));
        }
        Ok(())
    }
}

/// Convert a [`PixelView`] to `target` using frozen [`ColorServices`] (`CmsChoice::StrictNoIcc`
/// refuses arbitrary ICC conversions cleanly with [`Failure::MissingCms`]).
fn convert_pixel_view_with_src_format(
    src: PixelView<'_>,
    src_fmt: &PixelFormat,
    target: PixelFormat,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<PixelBuf> {
    let mut policy = ConvertPolicy::default()
        .with_depth(zenpixels_convert::DepthPolicy::Round)
        .with_gamut(zenpixels_convert::GamutPolicy::Clip);
    match eff.color_services.cms_choice() {
        CmsChoice::Use(binding) => {
            policy = policy.with_cms(Arc::clone(binding.backend()));
            report.cms_id = Some(binding.id().to_string());
            eff.trace_event(SelectionEvent::CmsUsed {
                id: binding.id().to_string(),
                revision: binding.revision().to_string(),
            });
        }
        CmsChoice::StrictNoIcc | CmsChoice::Inherit => {}
        _ => {}
    }

    let _output_plan = OutputPlan::build(
        src_fmt,
        &target,
        &policy,
        PeakSource::Declared,
        OutputSignaling::Strip,
    )
    .map_err(|e| {
        let failure = if matches!(eff.color_services.cms_choice(), CmsChoice::StrictNoIcc) {
            Failure::MissingCms
        } else {
            Failure::UnsupportedEffect
        };
        at!(EngineError::simple(
            Stage::Prepare,
            failure,
            "color conversion plan could not be built",
        )
        .with_cause(e))
    })?;

    let row_plan = RowPlan::build(src_fmt, &target, &policy, None).map_err(|e| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::UnsupportedEffect,
            "row conversion plan could not be built",
        )
        .with_cause(e))
    })?;

    let width = src.width();
    let height = src.height();
    let bpp = target.layout.bytes_per_pixel();
    let stride = (width as usize).checked_mul(bpp).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::ResourceLimit,
            "stride overflow"
        ))
    })?;
    let total = stride.checked_mul(height as usize).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::ResourceLimit,
            "buffer size overflow"
        ))
    })?;

    let mut dst = vec![0u8; total];
    for y in 0..height {
        if y & 63 == 0 {
            eff.check_stop(Stage::Execute)?;
        }
        let src_row = src.row(y);
        let dst_row = &mut dst[(y as usize) * stride..(y as usize + 1) * stride];
        row_plan.convert_row(src_row, dst_row, width).map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "row conversion failed",
            )
            .with_cause(e))
        })?;
    }

    PixelBuf::from_bytes(dst, width, height, target).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "converted pixel buffer validation failed",
        )
        .with_cause(e))
    })
}

fn bake_orientation_pixels(
    view: PixelView<'_>,
    orientation: Orientation,
    eff: &EffectiveRun,
) -> EngineResult<PixelBuf> {
    eff.check_stop(Stage::Execute)?;
    let (sw, sh) = (view.width(), view.height());
    let (dw, dh) = orientation.output_dimensions(sw, sh);
    let bpp = view.format().layout.bytes_per_pixel();
    let dst_stride = (dw as usize).checked_mul(bpp).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "orientation bake stride overflow",
        ))
    })?;
    let total = dst_stride.checked_mul(dh as usize).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "orientation bake buffer overflow",
        ))
    })?;
    let mut dst = Vec::new();
    dst.try_reserve_exact(total).map_err(|_| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "orientation bake allocation failed",
        ))
    })?;
    dst.resize(total, 0u8);
    for sy in 0..sh {
        if sy & 63 == 0 {
            eff.check_stop(Stage::Execute)?;
        }
        let srow = view.row(sy);
        for sx in 0..sw {
            let (dx, dy) = orientation.map_pixel_coordinates((sx, sy), (sw, sh));
            let s0 = (sx as usize) * bpp;
            let d0 = (dy as usize) * dst_stride + (dx as usize) * bpp;
            dst[d0..d0 + bpp].copy_from_slice(&srow[s0..s0 + bpp]);
        }
    }
    eff.check_stop(Stage::Execute)?;
    PixelBuf::from_bytes(dst, dw, dh, view.format().clone()).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "orientation bake PixelBuf construction failed",
        )
        .with_cause(e))
    })
}

fn calibrated_aim_seed(target: &MetricTarget) -> f32 {
    match target.metric {
        Metric::Ssimulacra2 => target.value.clamp(10.0, 99.0),
        Metric::ZensimA | Metric::ZensimB => target.value.clamp(10.0, 99.0),
        Metric::ButteraugliMax => (100.0 - target.value * 12.0).clamp(15.0, 98.0),
    }
}

/// Validate both [`PixelView::hdr`] sideband HDR signaling and [`PixelFormat`] SDR eligibility.
fn validate_sdr_verify_view(view: &PixelView<'_>, role: &str) -> EngineResult<()> {
    if !view.hdr().is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            alloc::format!("Coding::Verify refuses PixelView HDR metadata on {role}"),
        )));
    }
    validate_sdr_verify_interpretation(view.format(), role)
}

/// Parse an optional ICC v4.4 `cicp` tag directly from structurally validated ICC bytes
/// (works in both `cms` and `no_std` builds without relying on external CMS parsing).
fn parse_icc_embedded_cicp(icc: &[u8]) -> Option<(u8, u8, u8, bool)> {
    if icc.len() < 132 {
        return None;
    }
    let declared_size = u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize;
    let limit = declared_size.min(icc.len());
    let tag_count = u32::from_be_bytes([icc[128], icc[129], icc[130], icc[131]]) as usize;
    let table_end = tag_count.checked_mul(12)?.checked_add(132)?;
    if table_end > limit {
        return None;
    }
    for i in 0..tag_count {
        let base = 132 + i * 12;
        if &icc[base..base + 4] != b"cicp" {
            continue;
        }
        let off = u32::from_be_bytes([icc[base + 4], icc[base + 5], icc[base + 6], icc[base + 7]])
            as usize;
        let len = u32::from_be_bytes([icc[base + 8], icc[base + 9], icc[base + 10], icc[base + 11]])
            as usize;
        if len >= 12
            && off >= table_end
            && off.checked_add(len).is_some_and(|end| end <= limit)
            && &icc[off..off + 4] == b"cicp"
        {
            return Some((
                icc[off + 8],
                icc[off + 9],
                icc[off + 10],
                icc[off + 11] != 0,
            ));
        }
    }
    None
}

/// Structurally validate ICC profile bytes and verify they describe a full-range RGB/Gray SDR space.
fn validate_sdr_icc_bytes(icc: &[u8], role: &str) -> EngineResult<()> {
    if let Err(e) = zenpixels::validate_icc_structure(icc) {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidData,
            alloc::format!(
                "Coding::Verify refuses structurally invalid ICC profile on {role}: {e}"
            ),
        )));
    }
    match zenpixels::IccColorSpace::from_icc(icc) {
        Some(zenpixels::IccColorSpace::Rgb | zenpixels::IccColorSpace::Gray) => {}
        Some(_) => {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!("Coding::Verify refuses non-RGB/Gray ICC profile on {role}"),
            )));
        }
        None => {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidData,
                alloc::format!(
                    "Coding::Verify refuses unrecognized ICC profile color space on {role}"
                ),
            )));
        }
    }
    // Check embedded ICC v4.4 `cicp` tag directly in ALL feature builds (including without `cms`).
    if let Some((_primaries, tf, _matrix, full_range)) = parse_icc_embedded_cicp(icc) {
        if tf == 16 || tf == 18 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!(
                    "Coding::Verify refuses HDR ICC embedded CICP transfer {tf} on {role}"
                ),
            )));
        }
        if tf == 8 || tf == 2 || tf == 0 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!(
                    "Coding::Verify refuses ambiguous/linear/unspecified ICC CICP transfer {tf} on {role}"
                ),
            )));
        }
        if !full_range {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!("Coding::Verify requires full-range SDR ICC CICP on {role}"),
            )));
        }
    }
    #[cfg(not(feature = "cms"))]
    {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::MissingCms,
            alloc::format!(
                "Coding::Verify requires the cms feature to validate and transform embedded ICC profiles on {role}"
            ),
        )));
    }
    #[cfg(feature = "cms")]
    {
        let profile = moxcms::ColorProfile::new_from_slice(icc).map_err(|e| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidData,
                alloc::format!("Coding::Verify refuses malformed ICC profile on {role}: {e:?}"),
            ))
        })?;
        match profile.color_space {
            moxcms::DataColorSpace::Rgb | moxcms::DataColorSpace::Gray => {}
            other => {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify refuses non-RGB/Gray ICC data space {other:?} on {role}"
                    ),
                )));
            }
        }
        if let Some(cicp) = &profile.cicp {
            let tf = cicp.transfer_characteristics as u8;
            if tf == 16 || tf == 18 {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify refuses HDR ICC embedded CICP transfer {tf} on {role}"
                    ),
                )));
            }
            if tf == 8 || tf == 2 || tf == 0 {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify refuses ambiguous/linear/unspecified ICC CICP transfer {tf} on {role}"
                    ),
                )));
            }
            if !cicp.full_range {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!("Coding::Verify requires full-range SDR ICC CICP on {role}"),
                )));
            }
        }
        Ok(())
    }
}

/// Validate that `fmt` carries a known, full-packed, full-range SDR color interpretation
/// before allowing any `Coding::Verify` format-equality shortcut or conversion.
fn validate_sdr_verify_interpretation(fmt: &PixelFormat, role: &str) -> EngineResult<()> {
    if fmt.layout.packing != BitPacking::Full {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            alloc::format!("Coding::Verify {role} must use full-packed pixel layout"),
        )));
    }
    if fmt.color.is_hdr() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            alloc::format!("Coding::Verify is qualified for still SDR images only ({role} is HDR)"),
        )));
    }
    match &fmt.color {
        ColorSpace::Rgb {
            primaries,
            transfer,
            range,
            alpha,
            white,
        } => {
            if *primaries == zenpixels::Primaries::Unknown {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnknownInterpretation,
                    alloc::format!("Coding::Verify refuses unknown RGB primaries on {role}"),
                )));
            }
            if *transfer == Transfer::Unknown {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnknownInterpretation,
                    alloc::format!("Coding::Verify refuses unknown transfer on {role}"),
                )));
            }
            if matches!(transfer, Transfer::Pq | Transfer::Hlg(_)) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!("Coding::Verify refuses HDR transfer on SDR {role}"),
                )));
            }
            if *range != SignalRange::Full {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!("Coding::Verify requires full-range SDR pixels on {role}"),
                )));
            }
            if white.is_some() {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!("Coding::Verify refuses HDR DiffuseWhite anchor on SDR {role}"),
                )));
            }
            if *alpha == AlphaMode::Premultiplied {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify requires straight/opaque alpha (or AlphaPolicy::FlattenSrgb) on {role}"
                    ),
                )));
            }
            Ok(())
        }
        ColorSpace::Gray {
            transfer,
            range,
            alpha,
            white,
        } => {
            if *transfer == Transfer::Unknown {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnknownInterpretation,
                    alloc::format!("Coding::Verify refuses unknown grayscale transfer on {role}"),
                )));
            }
            if matches!(transfer, Transfer::Pq | Transfer::Hlg(_))
                || *range != SignalRange::Full
                || white.is_some()
                || *alpha == AlphaMode::Premultiplied
            {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify refuses unsupported grayscale SDR interpretation on {role}"
                    ),
                )));
            }
            Ok(())
        }
        ColorSpace::Icc {
            profile,
            color_channels,
            alpha,
            ..
        } => {
            if !matches!(*color_channels, 1 | 3) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!(
                        "Coding::Verify refuses {color_channels}-channel ICC color space on {role}"
                    ),
                )));
            }
            if *alpha == AlphaMode::Premultiplied {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    alloc::format!("Coding::Verify refuses premultiplied ICC alpha on {role}"),
                )));
            }
            validate_sdr_icc_bytes(profile, role)
        }
        _ => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            alloc::format!(
                "Coding::Verify requires RGB, Gray, or SDR ICC interpretation on {role}"
            ),
        ))),
    }
}

/// Returns `true` if `view` has meaningful alpha (`Straight` or `Premultiplied`) AND
/// at least one pixel has a non-opaque alpha value.
fn pixel_view_has_translucent_alpha(
    view: &PixelView<'_>,
    eff: &EffectiveRun,
) -> EngineResult<bool> {
    let fmt = view.format();
    if !matches!(
        fmt.color.alpha(),
        AlphaMode::Straight | AlphaMode::Premultiplied
    ) {
        return Ok(false);
    }
    let (w, h) = (view.width() as usize, view.height());
    match fmt.layout {
        SampleLayout::RGBA8 | SampleLayout::BGRA8 => {
            for y in 0..h {
                if y & 63 == 0 {
                    eff.check_stop(Stage::Admit)?;
                }
                let row = view.row(y);
                for px in row[..w * 4].as_chunks::<4>().0 {
                    if px[3] != 255 {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        SampleLayout::GRAYA8 => {
            for y in 0..h {
                if y & 63 == 0 {
                    eff.check_stop(Stage::Admit)?;
                }
                let row = view.row(y);
                for px in row[..w * 2].as_chunks::<2>().0 {
                    if px[1] != 255 {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        SampleLayout::RGBA16 => {
            for y in 0..h {
                if y & 63 == 0 {
                    eff.check_stop(Stage::Admit)?;
                }
                let row = view.row(y);
                for px in row[..w * 8].as_chunks::<8>().0 {
                    if u16::from_ne_bytes([px[6], px[7]]) != u16::MAX {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        SampleLayout::RGBAF32 => {
            for y in 0..h {
                if y & 63 == 0 {
                    eff.check_stop(Stage::Admit)?;
                }
                let row = view.row(y);
                for px in row[..w * 16].as_chunks::<16>().0 {
                    let a = f32::from_ne_bytes([px[12], px[13], px[14], px[15]]);
                    if !a.is_finite() || a.to_bits() != 1.0_f32.to_bits() {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        _ => Ok(true),
    }
}

/// Composite an alpha-bearing SDR view (honoring its resolved authoritative `src_fmt`) onto an
/// sRGB matte color in linear light, emitting opaque [`PixelFormat::RGB8_SRGB`].
fn flatten_alpha_onto_srgb_matte(
    src: PixelView<'_>,
    src_fmt: &PixelFormat,
    matte: [u8; 3],
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<PixelBuf> {
    let rgba_buf: Option<PixelBuf>;
    let rgba_view = if *src_fmt == PixelFormat::RGBA8_SRGB {
        src
    } else {
        let b =
            convert_pixel_view_with_src_format(src, src_fmt, PixelFormat::RGBA8_SRGB, eff, report)?;
        rgba_buf = Some(b);
        rgba_buf.as_ref().expect("just set").as_view()
    };

    let (w, h) = (rgba_view.width(), rgba_view.height());
    let stride = (w as usize).checked_mul(3).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "matte flatten stride overflow",
        ))
    })?;
    let total = stride.checked_mul(h as usize).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "matte flatten buffer overflow",
        ))
    })?;
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "matte flatten allocation failed",
        ))
    })?;
    out.resize(total, 0u8);

    let bg_lin = [
        linear_srgb::default::srgb_u8_to_linear(matte[0]),
        linear_srgb::default::srgb_u8_to_linear(matte[1]),
        linear_srgb::default::srgb_u8_to_linear(matte[2]),
    ];
    for y in 0..h {
        if y & 63 == 0 {
            eff.check_stop(Stage::Execute)?;
        }
        let srow = rgba_view.row(y);
        let drow = &mut out[(y as usize) * stride..(y as usize + 1) * stride];
        for x in 0..(w as usize) {
            let sp = &srow[x * 4..x * 4 + 4];
            let dp = &mut drow[x * 3..x * 3 + 3];
            let a = sp[3];
            if a == 255 {
                dp.copy_from_slice(&sp[..3]);
            } else if a == 0 {
                dp.copy_from_slice(&matte);
            } else {
                let af = f32::from(a) * (1.0 / 255.0);
                let inv_a = 1.0 - af;
                for c in 0..3 {
                    let fg = linear_srgb::default::srgb_u8_to_linear(sp[c]);
                    let lin = fg * af + bg_lin[c] * inv_a;
                    dp[c] = linear_srgb::default::linear_to_srgb_u8(lin);
                }
            }
        }
    }

    PixelBuf::from_bytes(out, w, h, PixelFormat::RGB8_SRGB).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "flattened PixelBuf validation failed",
        )
        .with_cause(e))
    })
}

fn active_compute_env() -> zencodec::estimate::ComputeEnvironment {
    #[cfg(feature = "std")]
    {
        zencodec::estimate::ComputeEnvironment::host()
    }
    #[cfg(not(feature = "std"))]
    {
        zencodec::estimate::ComputeEnvironment::conservative()
    }
}

/// Derive the expected decoded [`PixelFormat`] from probed [`ImageInfo`] (preserving bit depth
/// and channel count so 16-bit and wide decode buffers are accurately bounded) and any explicit
/// caller-requested `output_format`.
fn decode_pixel_format_from_info(
    info: Option<&ImageInfo>,
    requested: Option<&PixelFormat>,
    has_alpha: bool,
) -> PixelFormat {
    if let Some(req) = requested {
        return req.clone();
    }
    let wide = info
        .and_then(|i| i.source_color.bit_depth)
        .is_some_and(|b| b > 8);
    let ch = info.and_then(|i| i.source_color.channel_count);
    let alpha = has_alpha || info.is_some_and(|i| i.has_alpha);
    match (wide, ch, alpha) {
        (true, Some(1), false) => PixelFormat::GRAY16_SRGB,
        (false, Some(1), false) => PixelFormat::GRAY8_SRGB,
        (true, _, true) => PixelFormat::RGBA16_SRGB,
        (true, _, false) => PixelFormat::RGB16_SRGB,
        (false, _, true) => PixelFormat::RGBA8_SRGB,
        (false, _, false) => PixelFormat::RGB8_SRGB,
    }
}

/// Resolve the [`Fidelity`] that [`Coding`] selects for an ordinary still encode.
fn fidelity_for_coding(coding: &Coding) -> Option<Fidelity> {
    match coding {
        Coding::Auto | Coding::KeepBytes | Coding::ExactJpegRoundTrip => None,
        Coding::CalibratedQuality(q) => Some(Fidelity::codec_quality(*q)),
        Coding::QualityProfile { level, dpr } => {
            Some(Fidelity::codec_quality(level.calibrated_quality(*dpr)))
        }
        Coding::AimFor(t) | Coding::Verify(t) => {
            Some(Fidelity::codec_quality(calibrated_aim_seed(t)))
        }
        Coding::Lossless => Some(Fidelity::Lossless),
        Coding::LosslessOrKeep => Some(Fidelity::codec_quality(
            QualityLevel::Good.calibrated_quality(1.0),
        )),
    }
}

/// Qualified pure-Rust still decoder workspace bound for the actual decoded [`PixelFormat`]
/// and active host compute environment.
///
/// Only accepts concrete built-in pure-Rust still decoders (verified by `TypeId`) that report
/// a conservative worst-case `peak_memory_bytes_max()` from `estimate_decode_resources`.
/// Never promotes p50 `peak_memory_bytes_est()` or uncalibrated fallback multipliers into
/// `NativeBound::UpperBound`; unqualified / third-party decoders return [`NativeBound::Unknown`],
/// keeping [`WorkspacePolicy::Strict`] strictly honest.
fn qualified_still_decode_workspace(
    config: &dyn DynDecoderConfig,
    format: ImageFormat,
    width: u32,
    height: u32,
    pixel_format: &PixelFormat,
) -> NativeBound<u64> {
    if format == ImageFormat::Gif || !is_qualified_builtin_still_decoder(config) {
        return NativeBound::Unknown;
    }
    let chars = zencodec::estimate::ImageCharacteristics::new(
        width.max(1),
        height.max(1),
        pixel_format.clone(),
    );
    let est = config.estimate_decode_resources(&chars, &active_compute_env());
    let bpp = pixel_format.layout.bytes_per_pixel().max(1) as u64;
    let frame_bytes = u64::from(width.max(1))
        .saturating_mul(u64::from(height.max(1)))
        .saturating_mul(bpp);
    match est.peak_memory_bytes_max() {
        Some(peak_max) => NativeBound::UpperBound(peak_max.max(frame_bytes)),
        None => NativeBound::Unknown,
    }
}

/// Provider-specific header-probe workspace upper bound for built-in still decoders whose
/// `probe()` implementation allocates strictly bounded memory without unbounded decompression.
///
/// - `zenjpeg`: JPEG APP/COM markers are uncompressed slices of `encoded_len`, and `probe()` runs
///   with `PreserveConfig::all()`. Every retained APP/COM segment consumes at least 4 encoded
///   bytes (`FF <marker> 00 02`), so `extras.segments: Vec<PreservedSegment>` holds at most
///   `encoded_len / 4` entries. The bound sums:
///   - `Vec<PreservedSegment>` capacity doubling + transient realloc peak:
///     `≤ 3 × (encoded_len / 4) × size_of::<PreservedSegment>()`,
///   - per-segment allocator chunk rounding for tiny non-empty segment payloads:
///     `≤ (encoded_len / 4) × 32`,
///   - raw segment payload copies, parser `icc_profile`, reassembled `icc_cache`/`xmp_cache`
///     (+ transient reassembly buffer), `JpegInfo` clones, and `ImageInfo` `Arc<[u8]>` copies:
///     `≤ 6 × encoded_len`,
///   - fixed parser/Huffman/DQT/scanner tables and initial `Vec` minimum capacity (`512 KiB`).
/// - `zenbitmaps`: fixed BMP header struct (≤ 64 KiB, zero compressed metadata chunks).
/// - `zenpng` (inflates `iCCP` up to 1 MiB and accumulates decompressed `zTXt`/`iTXt` chunks),
///   `zenwebp`, `zengif`, `zenavif`, `zenjxl`, and third-party decoders return
///   [`NativeBound::Unknown`] so [`WorkspacePolicy::Strict`] never admits an unproven probe.
fn qualified_still_probe_bound(
    config: &dyn DynDecoderConfig,
    encoded_len: u64,
) -> NativeBound<u64> {
    let any = config.as_any();
    let _ = (any, encoded_len);
    #[cfg(feature = "jpeg")]
    if any.is::<zenjpeg::JpegDecoderConfig>() {
        let max_segments = encoded_len / 4;
        let seg_entry = core::mem::size_of::<zenjpeg::decoder::PreservedSegment>() as u64;
        let segment_table = max_segments.saturating_mul(3).saturating_mul(seg_entry);
        let segment_alloc_overhead = max_segments.saturating_mul(32);
        let metadata_copies = encoded_len.saturating_mul(6);
        return NativeBound::UpperBound(
            segment_table
                .saturating_add(segment_alloc_overhead)
                .saturating_add(metadata_copies)
                .saturating_add(512 * 1024),
        );
    }
    #[cfg(feature = "bitmaps-bmp")]
    if any.is::<zenbitmaps::BmpDecoderConfig>() {
        return NativeBound::UpperBound(64 * 1024);
    }
    NativeBound::Unknown
}

/// Qualified pure-Rust still encoder workspace bound for the actual emitted [`PixelFormat`],
/// quality/fidelity-resolved concrete encoder configuration, and active host compute environment.
///
/// Only accepts concrete built-in pure-Rust encoders (verified by `TypeId`) that report a
/// conservative worst-case `peak_memory_bytes_max()` from `estimate_encode_resources`.
/// Never promotes p50 `peak_memory_bytes_est()` or uncalibrated fallback multipliers into
/// `NativeBound::UpperBound`; unqualified / third-party encoders return [`NativeBound::Unknown`].
fn qualified_still_encode_workspace(
    config: &dyn zencodec::encode::DynEncoderConfig,
    _format: ImageFormat,
    width: u32,
    height: u32,
    pixel_format: &PixelFormat,
    fidelity: Option<Fidelity>,
) -> NativeBound<u64> {
    let chars = zencodec::estimate::ImageCharacteristics::new(
        width.max(1),
        height.max(1),
        pixel_format.clone(),
    );
    let compute = active_compute_env();
    let bpp = pixel_format.layout.bytes_per_pixel().max(1) as u64;
    let frame_bytes = u64::from(width.max(1))
        .saturating_mul(u64::from(height.max(1)))
        .saturating_mul(bpp);

    let est = estimate_qualified_builtin_encoder(config, &chars, &compute, fidelity);
    match est.and_then(|e| e.peak_memory_bytes_max()) {
        Some(peak_max) => NativeBound::UpperBound(peak_max.max(frame_bytes)),
        None => NativeBound::Unknown,
    }
}

fn estimate_qualified_builtin_encoder(
    config: &dyn zencodec::encode::DynEncoderConfig,
    chars: &zencodec::estimate::ImageCharacteristics,
    compute: &zencodec::estimate::ComputeEnvironment,
    fidelity: Option<Fidelity>,
) -> Option<zencodec::estimate::ResourceEstimate> {
    use zencodec::encode::EncoderConfig as _;
    let any = config.as_any();
    let _ = (any, chars, compute, fidelity);
    #[cfg(feature = "jpeg")]
    if let Some(cfg) = any.downcast_ref::<zenjpeg::JpegEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "png")]
    if let Some(cfg) = any.downcast_ref::<zenpng::PngEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "webp")]
    if let Some(cfg) = any.downcast_ref::<zenwebp::zencodec::WebpEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "gif")]
    if let Some(cfg) = any.downcast_ref::<zengif::GifEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "avif-encode")]
    if let Some(cfg) = any.downcast_ref::<zenavif::AvifEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "jxl-encode")]
    if let Some(cfg) = any.downcast_ref::<zenjxl::JxlEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    #[cfg(feature = "bitmaps-bmp")]
    if let Some(cfg) = any.downcast_ref::<zenbitmaps::BmpEncoderConfig>() {
        let resolved = match fidelity {
            Some(f) => cfg.clone().with_fidelity(f),
            None => cfg.clone(),
        };
        return Some(resolved.estimate_encode_resources(chars, compute));
    }
    None
}

fn is_qualified_builtin_still_decoder(config: &dyn DynDecoderConfig) -> bool {
    let any = config.as_any();
    let _ = any;
    #[cfg(feature = "jpeg")]
    if any.is::<zenjpeg::JpegDecoderConfig>() {
        return true;
    }
    #[cfg(feature = "png")]
    if any.is::<zenpng::PngDecoderConfig>() {
        return true;
    }
    #[cfg(feature = "webp")]
    if any.is::<zenwebp::zencodec::WebpDecoderConfig>() {
        return true;
    }
    #[cfg(feature = "avif-decode")]
    if any.is::<zenavif::AvifDecoderConfig>() {
        return true;
    }
    #[cfg(feature = "jxl-decode")]
    if any.is::<zenjxl::JxlDecoderConfig>() {
        return true;
    }
    #[cfg(feature = "bitmaps-bmp")]
    if any.is::<zenbitmaps::BmpDecoderConfig>() {
        return true;
    }
    false
}

/// Classify a boxed backend codec error while preserving typed [`StopReason`] (even when
/// backed by a consumed one-shot stop) and [`zencodec::LimitExceeded`] / `ErrorCategory::Resource`.
fn classify_backend_error(
    stage: Stage,
    default_failure: Failure,
    msg: &'static str,
    err: Box<dyn core::error::Error + Send + Sync>,
    eff: &EffectiveRun,
    report: Option<&RouteReport>,
) -> At<EngineError> {
    use enough::Stop as _;
    use zencodec::CodecErrorExt;
    let failure = if let Some(zencodec::ErrorCategory::Stopped(reason)) = err.error_category() {
        Failure::Stopped(reason)
    } else if err.limit_exceeded().is_some()
        || matches!(
            err.error_category(),
            Some(zencodec::ErrorCategory::Resource(_))
        )
    {
        Failure::ResourceLimit
    } else if let Err(reason) = eff.resources.pulse().check() {
        Failure::Stopped(reason)
    } else {
        default_failure
    };
    let mut e = EngineError::simple(stage, failure, msg).with_boxed_cause(err);
    if let Some(r) = report {
        e = e.with_diagnostics(r.clone(), eff.trace.clone());
    }
    at!(e)
}

/// Validate that [`Metadata`] does not carry HDR mastering/CLL/DiffuseWhite/CICP or invalid/non-SDR
/// ICC evidence before allowing `Coding::Verify`.
fn validate_sdr_verify_metadata(meta: &Metadata, role: &str) -> EngineResult<()> {
    if meta.content_light_level.is_some()
        || meta.mastering_display.is_some()
        || meta.diffuse_white.is_some()
    {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            alloc::format!(
                "Coding::Verify refuses HDR mastering/CLL/DiffuseWhite metadata on {role}"
            ),
        )));
    }
    if let Some(cicp) = meta.cicp {
        if cicp.transfer_characteristics == 16 || cicp.transfer_characteristics == 18 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!(
                    "Coding::Verify refuses HDR CICP transfer {} on {role}",
                    cicp.transfer_characteristics
                ),
            )));
        }
        if cicp.transfer_characteristics == 8 {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!("Coding::Verify refuses ambiguous linear CICP transfer on {role}"),
            )));
        }
        if !cicp.full_range {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                alloc::format!("Coding::Verify requires full-range SDR CICP metadata on {role}"),
            )));
        }
    }
    if let Some(icc) = meta.icc_profile.as_deref()
        && !icc.is_empty()
    {
        validate_sdr_icc_bytes(icc, role)?;
    }
    Ok(())
}

/// Validate probed [`zencodec::SourceColor`] before decoding for `Coding::Verify`.
fn validate_sdr_verify_source_color(sc: &zencodec::SourceColor, role: &str) -> EngineResult<()> {
    let mut meta = Metadata::none();
    meta.cicp = sc.cicp;
    meta.icc_profile = sc.icc_profile.clone();
    meta.content_light_level = sc.content_light_level;
    meta.mastering_display = sc.mastering_display;
    meta.diffuse_white = sc.diffuse_white;
    validate_sdr_verify_metadata(&meta, role)
}

/// Reconcile an authoritative ICC profile from [`zencodec::SourceColor`] onto a decoded
/// [`PixelFormat`] when the decoder emitted untagged RGB/Gray pixels.
fn effective_format_with_source_color(
    fmt: &PixelFormat,
    sc: &zencodec::SourceColor,
) -> EngineResult<PixelFormat> {
    let icc_is_authoritative = sc.color_authority.select(
        sc.icc_profile
            .as_deref()
            .filter(|b| !b.is_empty())
            .map(|_| true),
        sc.cicp.map(|_| false),
    ) == Some(true);
    if icc_is_authoritative && !matches!(fmt.color, ColorSpace::Icc { .. }) {
        let resolved = zencodec::helpers::pixel_format_for_decoded_pixels(fmt.layout, sc, None)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Admit,
                    Failure::InvalidData,
                    alloc::format!("failed to reconcile authoritative ICC profile: {e:?}"),
                ))
            })?;
        // Preserve the input view's exact alpha mode (e.g. Premultiplied vs Straight vs Opaque)
        // since `pixel_format_for_decoded_pixels` derives a default Straight mode from layout alone.
        Ok(PixelFormat::new(
            resolved.layout,
            resolved.color.with_alpha(fmt.color.alpha()),
        ))
    } else {
        Ok(fmt.clone())
    }
}

/// Reconcile an authoritative ICC profile from [`Metadata`] onto `fmt`.
fn effective_format_with_metadata(fmt: &PixelFormat, meta: &Metadata) -> EngineResult<PixelFormat> {
    if meta.icc_profile.as_deref().is_some_and(|b| !b.is_empty())
        && !matches!(fmt.color, ColorSpace::Icc { .. })
    {
        let mut sc = zencodec::SourceColor::default();
        sc.cicp = meta.cicp;
        sc.icc_profile = meta.icc_profile.clone();
        sc.content_light_level = meta.content_light_level;
        sc.mastering_display = meta.mastering_display;
        sc.diffuse_white = meta.diffuse_white;
        effective_format_with_source_color(fmt, &sc)
    } else {
        Ok(fmt.clone())
    }
}

/// Reconcile color-tagging fields on `meta` after pixels are converted to `target_fmt` so stale
/// source ICC/CICP/HDR tags are never passed to an encoder for converted pixels.
fn reconcile_converted_metadata(meta: &mut Metadata, target_fmt: &PixelFormat) {
    meta.icc_profile = None;
    meta.content_light_level = None;
    meta.mastering_display = None;
    meta.diffuse_white = None;
    if *target_fmt == PixelFormat::RGB8_SRGB || *target_fmt == PixelFormat::RGBA8_SRGB {
        meta.cicp = Some(zenpixels::Cicp::SRGB);
    } else {
        meta.cicp = None;
    }
}

/// Qualify an encoder quality point and resolve its post-quantization deduplication bucket +
/// effective encoder quality so flat calibration plateaus and integer VP8 quality rounding
/// (e.g. WebP generic q=85.0 and q=85.5 both rounding to VP8 integer quality 86) never spend
/// multiple Verify trials on identical encoder settings.
///
/// Unqualified third-party encoders without a known effective setting mapping are explicitly
/// refused rather than guessed from raw floats.
fn resolve_effective_verify_setting(
    config: &dyn zencodec::encode::DynEncoderConfig,
    q: f32,
) -> EngineResult<(u32, f32)> {
    config
        .try_fidelity_job(Fidelity::codec_quality(q))
        .map_err(|e| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "encoder refused calibrated quality dial point required by Coding::Verify",
            )
            .with_cause(e))
        })?;
    let any = config.as_any();
    let _ = any;
    #[cfg(feature = "webp")]
    if let Some(webp_cfg) = any.downcast_ref::<zenwebp::zencodec::WebpEncoderConfig>() {
        use zencodec::encode::EncoderConfig as _;
        let resolved = webp_cfg.clone().with_fidelity(Fidelity::codec_quality(q));
        let native_q = match resolved.inner() {
            zenwebp::EncoderConfig::Lossy(cfg) => cfg.quality,
            zenwebp::EncoderConfig::Lossless(cfg) => cfg.quality,
        };
        // Exact match to `zenwebp::LossyConfig::to_params`: VP8 quantizes quality to rounded u8.
        let quantized_u8 = native_q.clamp(0.0, 100.0).round() as u32;
        return Ok((quantized_u8, quantized_u8 as f32));
    }
    #[cfg(feature = "jpeg")]
    if let Some(jpeg_cfg) = any.downcast_ref::<zenjpeg::JpegEncoderConfig>() {
        use zencodec::encode::EncoderConfig as _;
        let resolved = jpeg_cfg.clone().with_fidelity(Fidelity::codec_quality(q));
        if let Some(Fidelity::Lossy(zencodec::encode::LossyTarget::CodecSpecificQuality(eff_q))) =
            resolved.resolved_target_fidelity()
        {
            let clamped = eff_q.clamp(0.0, 100.0);
            let key = (clamped * 100.0).round() as u32;
            return Ok((key, clamped));
        }
    }
    Err(at!(EngineError::simple(
        Stage::Admit,
        Failure::UnsupportedEffect,
        "encoder does not expose a qualified post-quantization quality setting for Coding::Verify",
    )))
}

fn intersect_opt_u32(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn intersect_opt_u64(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn intersect_limits(parent: &ResourceLimits, child: &ResourceLimits) -> ResourceLimits {
    let mut out = *parent;
    out.max_width = intersect_opt_u32(parent.max_width, child.max_width);
    out.max_height = intersect_opt_u32(parent.max_height, child.max_height);
    out.max_pixels = intersect_opt_u64(parent.max_pixels, child.max_pixels);
    out.max_input_bytes = intersect_opt_u64(parent.max_input_bytes, child.max_input_bytes);
    out.max_output_bytes = intersect_opt_u64(parent.max_output_bytes, child.max_output_bytes);
    out.max_memory_bytes = intersect_opt_u64(parent.max_memory_bytes, child.max_memory_bytes);
    out.max_frames = intersect_opt_u32(parent.max_frames, child.max_frames);
    out
}

fn check_dimensions(info: &ImageInfo, limits: &ResourceLimits) -> EngineResult<()> {
    if let Some(mw) = limits.max_width
        && info.width > mw
    {
        return Err(at!(EngineError::simple(
            Stage::Inspect,
            Failure::ResourceLimit,
            "probed image width exceeds max_width",
        )));
    }
    if let Some(mh) = limits.max_height
        && info.height > mh
    {
        return Err(at!(EngineError::simple(
            Stage::Inspect,
            Failure::ResourceLimit,
            "probed image height exceeds max_height",
        )));
    }
    if let Some(mp) = limits.max_pixels {
        let pixels = u64::from(info.width).saturating_mul(u64::from(info.height));
        if pixels > mp {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::ResourceLimit,
                "probed image pixel count exceeds max_pixels",
            )));
        }
    }
    Ok(())
}
