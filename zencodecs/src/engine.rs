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
    AlphaMode, DiffuseWhite, HlgRendering, Nits, Orientation, PeakSource, PixelBuf, PixelFormat,
    PixelView, Primaries,
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

impl Metric {
    /// Whether higher scalar values indicate better visual fidelity.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        match self {
            Self::Ssimulacra2 | Self::ZensimA | Self::ZensimB => true,
            Self::ButteraugliMax => false,
        }
    }

    /// Check whether `observed` satisfies `target`.
    #[must_use]
    pub fn satisfies(self, observed: f32, target: f32) -> bool {
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
#[derive(Clone, Debug, PartialEq)]
pub struct MetricTarget {
    /// Metric identifier.
    pub metric: Metric,
    /// Optional pinned scorer revision string.
    pub revision: Option<String>,
    /// Target scalar value (finite, within metric domain).
    pub value: f32,
}

impl MetricTarget {
    /// Construct a validated metric target.
    pub fn new(metric: Metric, value: f32) -> EngineResult<Self> {
        if !value.is_finite() {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "metric target value must be finite",
            )));
        }
        let valid = match metric {
            Metric::Ssimulacra2 | Metric::ZensimA | Metric::ZensimB => value <= 100.0,
            Metric::ButteraugliMax => value >= 0.0,
        };
        if !valid {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidRequest,
                "metric target value is outside metric domain",
            )));
        }
        Ok(Self {
            metric,
            revision: None,
            value,
        })
    }

    /// Pin an exact scorer revision string.
    #[must_use]
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }
}

/// Score-only scalar perceptual metric provider for [`Coding::Verify`].
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
    /// One-shot calibrated aim targeting a scalar perceptual metric (no verification loop).
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
                if matches!(self.chroma, ChromaPolicy::Require(ChromaSampling::Cs420 | ChromaSampling::Cs422)) {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Lossless conflicts with subsampled chroma requirement",
                    )));
                }
            }
            Coding::Verify(target) => {
                if !target.value.is_finite() {
                    return Err(at!(EngineError::simple(
                        Stage::Inspect,
                        Failure::InvalidRequest,
                        "Verify metric target must be finite",
                    )));
                }
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
    /// Verified output failed postcondition or metric convergence checks.
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
    /// Final measured scalar metric score when `Coding::Verify` ran.
    pub verified_score: Option<f32>,
    /// Number of verify attempts executed.
    pub verify_attempts: u32,
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
                        alloc::format!(
                            "{component} unknown native workspace refused by PolicySet"
                        ),
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
                        report.unknown_workspace_components.push(component.to_string());
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
                at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "decoder construction failed",
                )
                .with_boxed_cause(e)
                .with_diagnostics(self.report.clone(), self.effective.trace.clone()))
            })?;
        self.effective.check_stop(Stage::Execute)?;
        let decoded = decoder.decode().map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "decode execution failed",
            )
            .with_boxed_cause(e)
            .with_diagnostics(self.report.clone(), self.effective.trace.clone()))
        })?;

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
            Some(child) => self.routing.merge(child).map_err(|e| at!(EngineError::from(e)))?,
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
                at!(EngineError::simple(
                    Stage::Inspect,
                    Failure::InvalidData,
                    "decoder header probe failed",
                )
                .with_boxed_cause(e))
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
                at!(EngineError::simple(
                    Stage::Inspect,
                    Failure::InvalidData,
                    "decoder header probe failed",
                )
                .with_boxed_cause(e))
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
        let offers: Vec<DecodeOffer> = ranked
            .into_iter()
            .map(|(id, entry)| DecodeOffer {
                engine_id: self.engine_id,
                id,
                revision: entry.revision,
                format,
                config: Arc::clone(&entry.factory),
                workspace: NativeBound::Unknown,
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
            for (id, entry) in ranked {
                offers.push(EncodeOffer {
                    id,
                    revision: entry.revision,
                    format,
                    factory: Arc::clone(&entry.factory),
                    workspace: NativeBound::Unknown,
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
        let validated_offer = DecodeOffer {
            engine_id: self.engine_id,
            id: entry.id.clone(),
            revision: entry.revision,
            format: offer.format,
            config: Arc::clone(&entry.factory),
            workspace: NativeBound::Unknown,
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
        if let Err(err) =
            effective.admit_workspace(validated_offer.id.as_str(), validated_offer.workspace, &mut report)
        {
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

        let offers = self.encode_offers(intent, &envelope, run)?;
        let mut report = RouteReport::default();

        // Bake D4 orientation onto pixels when `OrientationMode::Bake` is requested on
        // non-identity metadata orientation (and reconcile metadata orientation to Identity).
        let baked_buf: Option<PixelBuf>;
        let mut effective_meta = metadata.clone();
        let oriented_view: PixelView<'_> = if intent.orientation == OrientationMode::Bake
            && metadata.orientation != Orientation::Identity
        {
            let b = bake_orientation_pixels(pixels, metadata.orientation)?;
            effective_meta.orientation = Orientation::Identity;
            baked_buf = Some(b);
            baked_buf.as_ref().expect("just set").as_view()
        } else {
            pixels
        };

        // Prepare color conversion if explicit target format requested.
        let converted_buf: Option<PixelBuf>;
        let encode_view: PixelView<'_> = match &intent.color {
            ColorTarget::Preserve => oriented_view,
            ColorTarget::ConvertTo(target_fmt) if target_fmt == oriented_view.format() => {
                oriented_view
            }
            ColorTarget::ConvertTo(target_fmt) => {
                let buf =
                    convert_pixel_view(oriented_view, target_fmt.clone(), &mut eff, &mut report)?;
                converted_buf = Some(buf);
                converted_buf.as_ref().expect("just set").as_view()
            }
        };

        // Dispatch `Coding::Verify(MetricTarget)` to the bounded score-only search loop.
        if let Coding::Verify(ref target) = intent.coding {
            return self.encode_pixels_verified(
                intent,
                target,
                encode_view,
                &effective_meta,
                &offers,
                eff,
                report,
            );
        }

        // Select top admissible offer respecting `WorkspacePolicy`.
        let mut selected = None;
        for offer in offers {
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
            encode_view,
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
                        has_exif_or_xmp || has_color_or_hdr || i.orientation != Orientation::Identity
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
        _report: &mut RouteReport,
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
                Coding::AimFor(t) => Some(Fidelity::codec_quality(calibrated_aim_seed(t))),
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
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::BackendFailure,
                "encoder instantiation failed",
            )
            .with_boxed_cause(e))
        })?;
        eff.check_stop(Stage::Execute)?;
        let encoded = encoder.encode(pixels).map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "encode execution failed",
            )
            .with_boxed_cause(e))
        })?;

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
        offers: &[EncodeOffer],
        mut eff: EffectiveRun,
        mut report: RouteReport,
    ) -> EngineResult<Completion<Encoded>> {
        // Gate Verify to still SDR images in v1 (Decision 5).
        if pixels.format().color.is_hdr() {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "Coding::Verify is qualified for still SDR images only",
            )));
        }
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

        let offer = offers.first().ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::NoBackend(ImageFormat::Jpeg),
                "no eligible encoder offer for Verify",
            ))
        })?;
        eff.admit_workspace(offer.id.as_str(), offer.workspace, &mut report)?;

        // Prepare reference raster in scorer's declared comparison format once.
        let cmp_fmt = reg.scorer.comparison_format();
        let ref_buf: Option<PixelBuf>;
        let ref_view = if *pixels.format() == cmp_fmt {
            pixels.clone()
        } else {
            let b = convert_pixel_view(pixels.clone(), cmp_fmt.clone(), &mut eff, &mut report)?;
            ref_buf = Some(b);
            ref_buf.as_ref().expect("just set").as_view()
        };

        // Deterministic bounded binary search over calibrated quality [5.0, 100.0].
        let max_attempts = reg.max_attempts.get();
        let mut lo = 5.0f32;
        let mut hi = 100.0f32;
        let mut q = calibrated_aim_seed(target);
        let mut best_passing: Option<(EncodeOutput, f32)> = None;

        for attempt in 1..=max_attempts {
            eff.check_stop(Stage::Verify)?;
            let encoded = self.run_single_encode(
                offer,
                intent,
                pixels.clone(),
                metadata,
                Some(q),
                &mut eff,
                &mut report,
            )?;

            // Decode candidate back through the same Engine + Run routing, policy, and limits.
            let cand_run = Run::new()
                .with_pulse(eff.resources.pulse().clone())
                .with_limits(*eff.resources.limits())
                .with_storage(eff.resources.storage().clone())
                .with_routing(&eff.routing)
                .with_policy_set(eff.policy)
                .with_color_services(eff.color_services.clone())
                .with_workspace_policy(eff.workspace_policy);
            let decoded = self.decode(&DecodeNeeds::still(), encoded.data(), &cand_run)?;
            let cand_raw = decoded.output.pixels();
            let cand_buf: Option<PixelBuf>;
            let cand_view = if *cand_raw.format() == cmp_fmt {
                cand_raw
            } else {
                let b = convert_pixel_view(cand_raw, cmp_fmt.clone(), &mut eff, &mut report)?;
                cand_buf = Some(b);
                cand_buf.as_ref().expect("just set").as_view()
            };

            let observed = reg
                .scorer
                .score(ref_view.clone(), cand_view, &eff.resources)?;
            eff.trace_event(SelectionEvent::MetricMeasured {
                metric: target.metric,
                target: target.value,
                observed,
                attempt,
            });
            report.verify_attempts = attempt;

            if target.metric.satisfies(observed, target.value) {
                best_passing = Some((encoded, observed));
                hi = q;
            } else {
                lo = q;
            }
            if (hi - lo) < 0.5 {
                break;
            }
            q = 0.5 * (lo + hi);
        }

        let (accepted, score) = best_passing.ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Verify,
                Failure::CorruptOutput,
                "Coding::Verify exhausted attempt cap without meeting metric target",
            )
            .with_diagnostics(report.clone(), eff.trace.clone()))
        })?;

        self.verify_preserve_bytes_readback(
            intent,
            offer.format,
            metadata,
            accepted.data(),
            &mut eff,
            &mut report,
        )?;

        report.route = Some(RouteKind::Raster);
        report.output_format = Some(offer.format);
        report.encoder = Some(offer.id.clone());
        report.verified_score = Some(score);
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
    /// selects the readback decoder through the active `Routing`, admits its workspace under `eff`,
    /// and verifies every requested EXIF/XMP/ICC byte payload survived verbatim.
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
        let Some((dec_id, dec_entry)) = ranked.into_iter().next() else {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::UnsupportedEffect,
                "MetadataPolicy::PreserveBytes requires an eligible routed decoder to verify byte-exact metadata retention",
            )));
        };
        eff.admit_workspace(dec_id.as_str(), NativeBound::Unknown, report)?;
        let mut probe_job = dec_entry.factory.dyn_job();
        probe_job.set_limits(*eff.resources.limits());
        probe_job.set_pulse(eff.resources.pulse().clone());
        let readback = probe_job.probe(encoded_bytes).map_err(|e| {
            at!(EngineError::simple(
                Stage::Verify,
                Failure::CorruptOutput,
                "encoded output failed MetadataPolicy::PreserveBytes readback probe",
            )
            .with_boxed_cause(e))
        })?;
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
fn convert_pixel_view(
    src: PixelView<'_>,
    target: PixelFormat,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<PixelBuf> {
    let mut policy = ConvertPolicy::default();
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
        src.format(),
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

    let row_plan = RowPlan::build(src.format(), &target, &policy, None).map_err(|e| {
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
    let stride = (width as usize)
        .checked_mul(bpp)
        .ok_or_else(|| at!(EngineError::simple(Stage::Prepare, Failure::ResourceLimit, "stride overflow")))?;
    let total = stride
        .checked_mul(height as usize)
        .ok_or_else(|| at!(EngineError::simple(Stage::Prepare, Failure::ResourceLimit, "buffer size overflow")))?;

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
) -> EngineResult<PixelBuf> {
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
        let srow = view.row(sy);
        for sx in 0..sw {
            let (dx, dy) = orientation.map_pixel_coordinates((sx, sy), (sw, sh));
            let s0 = (sx as usize) * bpp;
            let d0 = (dy as usize) * dst_stride + (dx as usize) * bpp;
            dst[d0..d0 + bpp].copy_from_slice(&srow[s0..s0 + bpp]);
        }
    }
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
