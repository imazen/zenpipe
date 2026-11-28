//! Opt-in score-only perceptual IQA [`MetricScorer`] adapters backed by [`zenpsy_api`].
//!
//! All adapters are strictly **score-only** (`MapRequest::None`): diagnostic diffmaps,
//! heatmaps, and per-scale arrays are never requested or allocated during scoring or
//! [`Coding::Verify`](crate::engine::Coding::Verify) loops.
//!
//! Each scorer implements [`MetricScorer::prepare_reference`] using owned, job-local
//! [`PreparedMetricScorer`] state so [`Coding::Verify`](crate::engine::Coding::Verify)
//! prepares the reference image **once** and reuses its precomputed pyramid/scratch
//! across every candidate trial with zero shared locks.

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use whereat::at;
use zencodec::JobResources;
use zenpixels::{PixelFormat, PixelView};
use zenpsy_api::{Backend, ErrorKind, EvalOpts, MetricError, MetricId, PreparedReference};

use crate::engine::{
    Engine, EngineError, EngineResult, Failure, Metric, MetricScorer, MetricScorerMetadata,
    PreparedMetricScorer, Stage,
};

/// Compute one 16-sample-aligned `f32` plane's byte size (`stride_samples * height * 4`).
fn aligned_plane_bytes_f32(width: u32, height: u32, align_samples: u64) -> u64 {
    let w = u64::from(width.max(8));
    let h = u64::from(height.max(8));
    let stride = w.div_ceil(align_samples).saturating_mul(align_samples);
    stride.saturating_mul(h).saturating_mul(4)
}

/// Build score-only [`EvalOpts`] propagating the run's cooperative cancellation pulse
/// and active-pixel limit. Never requests diagnostic maps.
fn score_only_eval_opts(resources: &JobResources) -> EvalOpts<'_> {
    let mut opts = EvalOpts::new().with_pulse(resources.pulse());
    if let Some(max_pixels) = resources.limits().max_pixels {
        opts = opts.with_max_pixels(max_pixels);
    }
    opts
}

/// Convert a [`zenpsy_api::MetricId`] into engine-portable [`MetricScorerMetadata`].
fn metadata_from_metric_id(id: &MetricId) -> MetricScorerMetadata {
    let imp = id.implementation();
    let backend = match imp.backend() {
        Backend::Cpu => "cpu",
        Backend::Gpu { api, .. } => api,
        _ => "unknown",
    };
    let config: Vec<(alloc::string::String, alloc::string::String)> = id
        .config()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    MetricScorerMetadata {
        family: id.family(),
        variant: id.variant(),
        reference: id.reference().to_string(),
        crate_name: imp.crate_name(),
        crate_version: imp.crate_version(),
        score_revision: imp.score_revision(),
        backend,
        build: imp.build(),
        config,
    }
}

/// Map a typed [`zenpsy_api::MetricError`] into a traced [`EngineError`], preserving
/// stop reasons, resource limits, unsupported/invalid inputs, and the typed cause.
fn map_metric_error(stage: Stage, err: whereat::At<MetricError>) -> whereat::At<EngineError> {
    let (metric_err, _trace) = err.decompose();
    let failure = match metric_err.kind() {
        ErrorKind::Stopped(reason) => Failure::Stopped(reason),
        ErrorKind::Resource => Failure::ResourceLimit,
        ErrorKind::InvalidConfig | ErrorKind::NoMap => Failure::InvalidRequest,
        ErrorKind::UnsupportedInput | ErrorKind::InvalidInput | ErrorKind::DimensionMismatch => {
            Failure::UnsupportedEffect
        }
        ErrorKind::Internal => Failure::BackendFailure,
        _ => Failure::BackendFailure,
    };
    let msg = metric_err.to_string();
    at!(EngineError::simple(stage, failure, msg).with_cause(metric_err))
}

// ═══════════════════════════════════════════════════════════════════════════
// Zensim-A (slim zensim rev-7)
// ═══════════════════════════════════════════════════════════════════════════

/// Score-only [`MetricScorer`] for adopted slim **`zensim-a`** (`ZensimProfile::A`, rev-7).
#[cfg(feature = "iqa-zensim")]
#[derive(Clone, Copy, Debug, Default)]
pub struct ZensimAScorer;

#[cfg(feature = "iqa-zensim")]
impl ZensimAScorer {
    /// Canonical revision token matching `zensim::ZensimMetric::new().id().score_revision()`.
    pub const REVISION: &'static str = "zensim-a.r7";

    /// Construct a new slim `zensim-a` scorer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(feature = "iqa-zensim")]
struct PreparedZensimAScorer {
    prepared: PreparedReference<zensim::PreparedZensim>,
}

#[cfg(feature = "iqa-zensim")]
impl PreparedMetricScorer for PreparedZensimAScorer {
    fn score_candidate_f64(
        &mut self,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let opts = score_only_eval_opts(resources);
        let eval = self
            .prepared
            .score(&candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }
}

#[cfg(feature = "iqa-zensim")]
impl MetricScorer for ZensimAScorer {
    fn metric(&self) -> Metric {
        Metric::ZensimA
    }

    fn revision(&self) -> &'static str {
        Self::REVISION
    }

    fn comparison_format(&self) -> PixelFormat {
        PixelFormat::RGBA8_SRGB
    }

    fn score(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f32> {
        self.score_f64(reference, candidate, resources)
            .map(|s| s as f32)
    }

    fn score_f64(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let mut metric = zensim::Zensim::new(zensim::ZensimProfile::A);
        let opts = score_only_eval_opts(resources);
        let eval = zenpsy_api::score(&mut metric, &reference, &candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }

    fn prepare_reference(
        &self,
        reference: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<Option<Box<dyn PreparedMetricScorer>>> {
        let mut metric = zensim::Zensim::new(zensim::ZensimProfile::A);
        let opts = score_only_eval_opts(resources);
        let prepared = zenpsy_api::prepare_reference(&mut metric, &reference, opts)
            .map_err(|e| map_metric_error(Stage::Prepare, e))?;
        Ok(Some(Box::new(PreparedZensimAScorer { prepared })))
    }

    fn estimated_scratch_bytes(&self, width: u32, height: u32) -> u64 {
        // 16-sample-aligned 4-scale XYB reference pyramid + candidate planes + edge/blur scratch.
        let full_plane = aligned_plane_bytes_f32(width, height, 16);
        full_plane
            .saturating_mul(24)
            .saturating_add(2 * 1024 * 1024)
    }

    fn metadata(&self) -> Option<MetricScorerMetadata> {
        let id = zenpsy_api::Metric::id(&zensim::Zensim::new(zensim::ZensimProfile::A));
        Some(metadata_from_metric_id(&id))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SSIMULACRA2 (`fast-ssim2`)
// ═══════════════════════════════════════════════════════════════════════════

/// Score-only [`MetricScorer`] for SSIMULACRA2 (`fast-ssim2`).
#[cfg(feature = "iqa-ssimulacra2")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Ssimulacra2Scorer;

#[cfg(feature = "iqa-ssimulacra2")]
impl Ssimulacra2Scorer {
    /// Canonical revision token matching `fast_ssim2::Ssimulacra2Metric::new().id().score_revision()`.
    pub const REVISION: &'static str = "ssimulacra2.r1";

    /// Construct a new SSIMULACRA2 scorer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(feature = "iqa-ssimulacra2")]
struct PreparedSsimulacra2Scorer {
    prepared: PreparedReference<fast_ssim2::PreparedSsimulacra2>,
}

#[cfg(feature = "iqa-ssimulacra2")]
impl PreparedMetricScorer for PreparedSsimulacra2Scorer {
    fn score_candidate_f64(
        &mut self,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let opts = score_only_eval_opts(resources);
        let eval = self
            .prepared
            .score(&candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }
}

#[cfg(feature = "iqa-ssimulacra2")]
impl MetricScorer for Ssimulacra2Scorer {
    fn metric(&self) -> Metric {
        Metric::Ssimulacra2
    }

    fn revision(&self) -> &'static str {
        Self::REVISION
    }

    fn comparison_format(&self) -> PixelFormat {
        PixelFormat::RGBA8_SRGB
    }

    fn score(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f32> {
        self.score_f64(reference, candidate, resources)
            .map(|s| s as f32)
    }

    fn score_f64(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let mut metric = fast_ssim2::Ssimulacra2Metric::new();
        let opts = score_only_eval_opts(resources);
        let eval = zenpsy_api::score(&mut metric, &reference, &candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }

    fn prepare_reference(
        &self,
        reference: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<Option<Box<dyn PreparedMetricScorer>>> {
        let mut metric = fast_ssim2::Ssimulacra2Metric::new();
        let opts = score_only_eval_opts(resources);
        let prepared = zenpsy_api::prepare_reference(&mut metric, &reference, opts)
            .map_err(|e| map_metric_error(Stage::Prepare, e))?;
        Ok(Some(Box::new(PreparedSsimulacra2Scorer { prepared })))
    }

    fn estimated_scratch_bytes(&self, width: u32, height: u32) -> u64 {
        // 16-sample-aligned 6-scale XYB reference + candidate planes + Gaussian blur/mu/sigma scratch.
        let full_plane = aligned_plane_bytes_f32(width, height, 16);
        full_plane
            .saturating_mul(28)
            .saturating_add(2 * 1024 * 1024)
    }

    fn metadata(&self) -> Option<MetricScorerMetadata> {
        let id = zenpsy_api::Metric::id(&fast_ssim2::Ssimulacra2Metric::new());
        Some(metadata_from_metric_id(&id))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Butteraugli Max (`butteraugli` score-only zero-diffmap reduction)
// ═══════════════════════════════════════════════════════════════════════════

/// Score-only [`MetricScorer`] for Butteraugli max distance (`butteraugli`).
///
/// Uses Butteraugli's zero-diffmap scalar reduction and strictly refuses translucent alpha
/// unless explicitly flattened upstream.
#[cfg(feature = "iqa-butteraugli")]
#[derive(Clone, Copy, Debug, Default)]
pub struct ButteraugliMaxScorer;

#[cfg(feature = "iqa-butteraugli")]
impl ButteraugliMaxScorer {
    /// Canonical revision token matching `butteraugli::ButteraugliMetric::new().id().score_revision()`.
    pub const REVISION: &'static str = "butteraugli.r1";

    /// Construct a new Butteraugli max scorer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(feature = "iqa-butteraugli")]
struct PreparedButteraugliMaxScorer {
    prepared: PreparedReference<butteraugli::PreparedButteraugli>,
}

#[cfg(feature = "iqa-butteraugli")]
impl PreparedMetricScorer for PreparedButteraugliMaxScorer {
    fn score_candidate_f64(
        &mut self,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let opts = score_only_eval_opts(resources);
        let eval = self
            .prepared
            .score(&candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }
}

#[cfg(feature = "iqa-butteraugli")]
impl MetricScorer for ButteraugliMaxScorer {
    fn metric(&self) -> Metric {
        Metric::ButteraugliMax
    }

    fn revision(&self) -> &'static str {
        Self::REVISION
    }

    fn comparison_format(&self) -> PixelFormat {
        // Straight RGBA8_SRGB preserves alpha evidence: opaque images (a == 255) score directly
        // while genuinely translucent inputs are explicitly refused by Butteraugli rather than
        // silently stripped.
        PixelFormat::RGBA8_SRGB
    }

    fn score(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f32> {
        self.score_f64(reference, candidate, resources)
            .map(|s| s as f32)
    }

    fn score_f64(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<f64> {
        let mut metric = butteraugli::ButteraugliMetric::new();
        let opts = score_only_eval_opts(resources);
        let eval = zenpsy_api::score(&mut metric, &reference, &candidate, opts)
            .map_err(|e| map_metric_error(Stage::Verify, e))?;
        Ok(eval.score())
    }

    fn prepare_reference(
        &self,
        reference: PixelView<'_>,
        resources: &JobResources,
    ) -> EngineResult<Option<Box<dyn PreparedMetricScorer>>> {
        let mut metric = butteraugli::ButteraugliMetric::new();
        let opts = score_only_eval_opts(resources);
        let prepared = zenpsy_api::prepare_reference(&mut metric, &reference, opts)
            .map_err(|e| map_metric_error(Stage::Prepare, e))?;
        Ok(Some(Box::new(PreparedButteraugliMaxScorer { prepared })))
    }

    fn estimated_scratch_bytes(&self, width: u32, height: u32) -> u64 {
        // Butteraugli pads every plane to a 16-sample stride and simultaneously holds:
        // - 12 full-res + half-res reference PsychoImage/mask planes,
        // - 10 candidate PsychoImage planes,
        // - 3 AC working planes + 3 candidate linear RGB planes,
        // - Malta/blur temporaries and idle pool storage.
        let full_plane = aligned_plane_bytes_f32(width, height, 16);
        let half_w = width.max(8).div_ceil(2);
        let half_h = height.max(8).div_ceil(2);
        let half_plane = aligned_plane_bytes_f32(half_w, half_h, 16);
        full_plane
            .saturating_mul(36)
            .saturating_add(half_plane.saturating_mul(24))
            .saturating_add(4 * 1024 * 1024)
    }

    fn metadata(&self) -> Option<MetricScorerMetadata> {
        let id = zenpsy_api::Metric::id(&butteraugli::ButteraugliMetric::new());
        Some(metadata_from_metric_id(&id))
    }
}

/// Register every compiled-in score-only perceptual IQA [`MetricScorer`] on `engine`
/// with the given per-job verification attempt cap.
#[allow(unused_mut)]
pub fn register_enabled_scorers(
    mut engine: Engine,
    max_attempts: NonZeroU32,
) -> EngineResult<Engine> {
    #[cfg(feature = "iqa-zensim")]
    {
        engine = engine.with_metric_scorer(Arc::new(ZensimAScorer::new()), max_attempts)?;
    }
    #[cfg(feature = "iqa-ssimulacra2")]
    {
        engine = engine.with_metric_scorer(Arc::new(Ssimulacra2Scorer::new()), max_attempts)?;
    }
    #[cfg(feature = "iqa-butteraugli")]
    {
        engine = engine.with_metric_scorer(Arc::new(ButteraugliMaxScorer::new()), max_attempts)?;
    }
    Ok(engine)
}
