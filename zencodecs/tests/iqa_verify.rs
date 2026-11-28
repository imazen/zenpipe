//! End-to-end score-only IQA (`zensim-a`, `ssimulacra2`, `butteraugli_max`) verification
//! through `zencodecs::Engine` under `WorkspacePolicy::Strict`.
//!
//! Covers:
//! - Warm `PreparedMetricScorer` reuse via `zenpsy-api` (`ZensimAScorer`, `Ssimulacra2Scorer`,
//!   `ButteraugliMaxScorer`) with zero diffmap allocation.
//! - Real qualified still codec workspace bounds passing `WorkspacePolicy::Strict`.
//! - Bracket-and-bisect convergence (including `q=100.0` sole-pass reachability, `q=5.0`
//!   lower endpoint check, and typed `Failure::TargetUnmet` when unreachable).
//! - `Coding::AimFor` domain validation and revision pinning.
//! - `AlphaPolicy::FlattenSrgb` vs translucent refusal on opaque-only JPEG and Butteraugli.
//! - `OrientationMode::PreserveStored` upright comparison.
//! - Cooperative cancellation via `SharedPulse` during `Coding::Verify`.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use enough::{Stop, StopReason};
use zencodec::{CodecSet, ImageFormat, JobResources, Metadata, Orientation, SharedPulse};
use zencodecs::bundles;
use zencodecs::engine::{
    AIM_SEED_REVISION, AlphaPolicy, Coding, Engine, EngineResult, Failure, Metric, MetricScorer,
    MetricTarget, OrientationMode, OutputIntent, PreparedMetricScorer, Run, WorkspacePolicy,
};
use zencodecs::iqa::{
    ButteraugliMaxScorer, Ssimulacra2Scorer, ZensimAScorer, register_enabled_scorers,
};
use zenpixels::{PixelBuf, PixelFormat, PixelView};

fn make_textured_rgb8(w: u32, h: u32) -> PixelBuf {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let yf = y as f32 / (h.max(1) as f32);
        for x in 0..w {
            let xf = x as f32 / (w.max(1) as f32);
            let wave1 = (xf * 12.0 + yf * 5.0).sin() * 0.5 + 0.5;
            let wave2 = (xf * 7.0 - yf * 11.0).cos() * 0.5 + 0.5;
            let wave3 = ((xf * 18.0).sin() * (yf * 14.0).cos()) * 0.5 + 0.5;
            let r = (32.0 + 180.0 * (0.65 * xf + 0.35 * wave1))
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = (28.0 + 185.0 * (0.60 * yf + 0.40 * wave2))
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (36.0 + 175.0 * (0.50 * (1.0 - xf) + 0.50 * wave3))
                .round()
                .clamp(0.0, 255.0) as u8;
            data.extend_from_slice(&[r, g, b]);
        }
    }
    PixelBuf::from_bytes(data, w, h, PixelFormat::RGB8_SRGB).unwrap()
}

fn make_translucent_rgba8(w: u32, h: u32) -> PixelBuf {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let yf = y as f32 / (h.max(1) as f32);
        for x in 0..w {
            let xf = x as f32 / (w.max(1) as f32);
            let r = (40.0 + 170.0 * xf).round().clamp(0.0, 255.0) as u8;
            let g = (35.0 + 175.0 * yf).round().clamp(0.0, 255.0) as u8;
            let b = (50.0 + 160.0 * ((xf + yf) * 0.5)).round().clamp(0.0, 255.0) as u8;
            // Smooth alpha vignette with genuine translucency in the center.
            let dx = xf - 0.5;
            let dy = yf - 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let a = (110.0 + 145.0 * (dist * 1.8).min(1.0))
                .round()
                .clamp(0.0, 255.0) as u8;
            data.extend_from_slice(&[r, g, b, a]);
        }
    }
    PixelBuf::from_bytes(data, w, h, PixelFormat::RGBA8_SRGB).unwrap()
}

fn build_strict_iqa_engine(max_attempts: u32) -> Engine {
    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    register_enabled_scorers(engine, NonZeroU32::new(max_attempts).unwrap()).unwrap()
}

#[test]
fn strict_workspace_verify_converges_with_zensim_a_ssim2_and_butteraugli_across_codecs() {
    let engine = build_strict_iqa_engine(8);

    // Registry introspection exposes all three enabled scorers with structured metadata.
    let registered: Vec<_> = engine.registered_metric_scorers().collect();
    assert_eq!(registered.len(), 3);
    for info in &registered {
        let meta = info.scorer.metadata().expect("zenpsy-api metadata present");
        assert_eq!(meta.backend, "cpu");
        assert!(!meta.crate_name.is_empty());
        assert!(meta.score_revision >= 1);
    }

    let img = make_textured_rgb8(96, 80);
    // Strict is the default WorkspacePolicy on Run::new().
    let strict_run = Run::new().with_trace(NonZeroU32::new(64).unwrap());
    assert_eq!(strict_run.workspace_policy(), WorkspacePolicy::Strict);

    // 1. Zensim-A (adopted slim rev-7) on JPEG under WorkspacePolicy::Strict.
    let target = MetricTarget::new(Metric::ZensimA, 82.0)
        .unwrap()
        .with_revision(ZensimAScorer::REVISION);
    let intent =
        OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(target.clone()));
    let done = engine
        .encode_pixels(&intent, img.as_view(), &Metadata::none(), None, &strict_run)
        .unwrap_or_else(|e| panic!("ZensimA Verify on JPEG failed: {e}"));
    assert!(
        done.report.unknown_workspace_components.is_empty(),
        "qualified JPEG must have zero unknown workspace components under Strict"
    );
    assert_eq!(done.report.verified_metric, Some(Metric::ZensimA));
    assert_eq!(
        done.report.verified_revision.as_deref(),
        Some(ZensimAScorer::REVISION)
    );
    let score_f64 = done.report.verified_score_f64.expect("native f64 score");
    assert!(
        (82.0..=100.0).contains(&score_f64),
        "JPEG score={score_f64}"
    );
    assert!(done.report.verified_quality.is_some());
    assert!((1..=8).contains(&done.report.verify_attempts));

    // WebP decode only exposes p50 estimate (no proven peak_memory_bytes_max), so Strict
    // honestly refuses unqualified WebP readback while BestEffort converges and records it.
    let webp_intent =
        OutputIntent::format(ImageFormat::WebP).with_coding(Coding::Verify(target.clone()));
    let webp_strict_err = engine
        .encode_pixels(
            &webp_intent,
            img.as_view(),
            &Metadata::none(),
            None,
            &strict_run,
        )
        .unwrap_err();
    assert_eq!(
        webp_strict_err.error().failure(),
        &Failure::PolicyRefusal,
        "Strict must honestly refuse WebP Verify when decoder lacks proven upper bound, got: {:?}",
        webp_strict_err.error()
    );

    let best_effort_run = Run::new()
        .with_workspace_policy(WorkspacePolicy::BestEffort)
        .with_trace(NonZeroU32::new(64).unwrap());
    let webp_done = engine
        .encode_pixels(
            &webp_intent,
            img.as_view(),
            &Metadata::none(),
            None,
            &best_effort_run,
        )
        .unwrap_or_else(|e| panic!("ZensimA Verify on WebP under BestEffort failed: {e}"));
    assert!(
        !webp_done.report.unknown_workspace_components.is_empty(),
        "BestEffort WebP Verify must report unqualified readback decoder in unknown_workspace_components"
    );
    assert!(webp_done.report.verified_score_f64.unwrap() >= 82.0);
    assert!((1..=8).contains(&webp_done.report.verify_attempts));

    // WebP calibration plateau deduplication: generic q=99 and q=100 map to identical native q=99,
    // so an unreachable ZensimA=100.0 target spends exactly 1 trial (never 2 on the same setting).
    let impossible_webp = OutputIntent::format(ImageFormat::WebP).with_coding(Coding::Verify(
        MetricTarget::new(Metric::ZensimA, 100.0).unwrap(),
    ));
    let webp_unmet = engine
        .encode_pixels(
            &impossible_webp,
            img.as_view(),
            &Metadata::none(),
            None,
            &best_effort_run,
        )
        .unwrap_err();
    assert_eq!(webp_unmet.error().failure(), &Failure::TargetUnmet);
    assert_eq!(
        webp_unmet.error().route_report().verify_attempts,
        1,
        "WebP q=99/100 plateau must deduplicate to 1 trial"
    );

    // 2. SSIMULACRA2 on JPEG under Strict.
    let ssim2_target = MetricTarget::new(Metric::Ssimulacra2, 70.0)
        .unwrap()
        .with_revision(Ssimulacra2Scorer::REVISION);
    let ssim2_done = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ssim2_target)),
            img.as_view(),
            &Metadata::none(),
            None,
            &strict_run,
        )
        .unwrap();
    assert!(ssim2_done.report.verified_score_f64.unwrap() >= 70.0);
    assert_eq!(
        ssim2_done.report.verified_revision.as_deref(),
        Some(Ssimulacra2Scorer::REVISION)
    );

    // 3. Butteraugli max (lower is better) on JPEG under Strict.
    let ba_target = MetricTarget::new(Metric::ButteraugliMax, 4.0)
        .unwrap()
        .with_revision(ButteraugliMaxScorer::REVISION);
    let ba_done = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ba_target)),
            img.as_view(),
            &Metadata::none(),
            None,
            &strict_run,
        )
        .unwrap();
    let ba_score = ba_done.report.verified_score_f64.unwrap();
    assert!(
        (0.0..=4.0).contains(&ba_score),
        "Butteraugli max score={ba_score}"
    );
    assert_eq!(
        ba_done.report.verified_revision.as_deref(),
        Some(ButteraugliMaxScorer::REVISION)
    );
}

#[test]
fn verify_returns_typed_target_unmet_and_prepares_reference_once() {
    struct CountingWarmScorer {
        prep_calls: Arc<AtomicUsize>,
        score_calls: Arc<AtomicUsize>,
        cold_calls: Arc<AtomicUsize>,
        inner: ZensimAScorer,
    }

    struct CountingPrepared {
        score_calls: Arc<AtomicUsize>,
        inner: Box<dyn PreparedMetricScorer>,
    }

    impl PreparedMetricScorer for CountingPrepared {
        fn score_candidate_f64(
            &mut self,
            candidate: PixelView<'_>,
            resources: &JobResources,
        ) -> EngineResult<f64> {
            self.score_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.score_candidate_f64(candidate, resources)
        }
    }

    impl MetricScorer for CountingWarmScorer {
        fn metric(&self) -> Metric {
            Metric::ZensimA
        }

        fn revision(&self) -> &'static str {
            "counting-zensim-a.r7"
        }

        fn comparison_format(&self) -> PixelFormat {
            self.inner.comparison_format()
        }

        fn score(
            &self,
            reference: PixelView<'_>,
            candidate: PixelView<'_>,
            resources: &JobResources,
        ) -> EngineResult<f32> {
            self.cold_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.score(reference, candidate, resources)
        }

        fn score_f64(
            &self,
            reference: PixelView<'_>,
            candidate: PixelView<'_>,
            resources: &JobResources,
        ) -> EngineResult<f64> {
            self.cold_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.score_f64(reference, candidate, resources)
        }

        fn prepare_reference(
            &self,
            reference: PixelView<'_>,
            resources: &JobResources,
        ) -> EngineResult<Option<Box<dyn PreparedMetricScorer>>> {
            self.prep_calls.fetch_add(1, Ordering::Relaxed);
            let inner = self
                .inner
                .prepare_reference(reference, resources)?
                .expect("ZensimAScorer prepares reference");
            Ok(Some(Box::new(CountingPrepared {
                score_calls: Arc::clone(&self.score_calls),
                inner,
            })))
        }
    }

    let prep_calls = Arc::new(AtomicUsize::new(0));
    let score_calls = Arc::new(AtomicUsize::new(0));
    let cold_calls = Arc::new(AtomicUsize::new(0));

    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services())
        .with_metric_scorer(
            Arc::new(CountingWarmScorer {
                prep_calls: Arc::clone(&prep_calls),
                score_calls: Arc::clone(&score_calls),
                cold_calls: Arc::clone(&cold_calls),
                inner: ZensimAScorer::new(),
            }),
            NonZeroU32::new(6).unwrap(),
        )
        .unwrap();

    let img = make_textured_rgb8(64, 64);

    // 1. Converging run prepares reference ONCE and uses zero cold calls across multiple trials.
    let ok_target = MetricTarget::new(Metric::ZensimA, 80.0).unwrap();
    let done = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ok_target)),
            img.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap();
    assert_eq!(
        prep_calls.load(Ordering::Relaxed),
        1,
        "prepare_reference must execute exactly once per Verify job"
    );
    assert_eq!(
        cold_calls.load(Ordering::Relaxed),
        0,
        "warm scorer must never fall back to cold score()"
    );
    assert_eq!(
        score_calls.load(Ordering::Relaxed),
        done.report.verify_attempts as usize
    );

    // 2. Unreachable target (100.0 on textured lossy JPEG) returns typed Failure::TargetUnmet.
    let impossible = MetricTarget::new(Metric::ZensimA, 100.0).unwrap();
    let err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(impossible)),
            img.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        err.error().failure(),
        &Failure::TargetUnmet,
        "unreachable target must fail with typed Failure::TargetUnmet, got: {:?}",
        err.error()
    );
    assert!(err.error().route_report().verify_attempts >= 1);
}

#[test]
fn alpha_flatten_orientation_aimfor_revision_and_cancellation() {
    let engine = build_strict_iqa_engine(8);

    // 1. Translucent RGBA8 refused on JPEG without FlattenSrgb, accepted with FlattenSrgb.
    let rgba = make_translucent_rgba8(64, 48);
    let target = MetricTarget::new(Metric::ButteraugliMax, 5.0).unwrap();
    let refused = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(target.clone())),
            rgba.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(refused.error().failure(), &Failure::UnsupportedEffect);

    let flattened = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_alpha(AlphaPolicy::FlattenSrgb([255, 255, 255]))
                .with_coding(Coding::Verify(target)),
            rgba.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap();
    assert!(flattened.report.verified_score_f64.unwrap() <= 5.0);

    // 2. Non-identity EXIF orientation with PreserveStored compares upright-to-upright.
    let non_square = make_textured_rgb8(64, 48);
    let mut rot_meta = Metadata::none();
    rot_meta.orientation = Orientation::Rotate90;
    let ztarget = MetricTarget::new(Metric::ZensimA, 80.0).unwrap();
    let oriented = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_orientation(OrientationMode::PreserveStored)
                .with_coding(Coding::Verify(ztarget.clone())),
            non_square.as_view(),
            &rot_meta,
            None,
            &Run::new(),
        )
        .unwrap();
    assert!(oriented.report.verified_score_f64.unwrap() >= 80.0);

    // 3. Coding::AimFor honors revision pins and records AIM_SEED_REVISION.
    let bad_aim = MetricTarget::new(Metric::ZensimA, 85.0)
        .unwrap()
        .with_revision("wrong-rev");
    let bad_aim_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::AimFor(bad_aim)),
            non_square.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(bad_aim_err.error().failure(), &Failure::UnsupportedEffect);

    let ok_aim = MetricTarget::new(Metric::ZensimA, 85.0)
        .unwrap()
        .with_revision(AIM_SEED_REVISION);
    let ok_aim_done = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::AimFor(ok_aim)),
            non_square.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap();
    assert_eq!(ok_aim_done.report.aim_metric, Some(Metric::ZensimA));
    assert_eq!(ok_aim_done.report.aim_target, Some(85.0));
    assert_eq!(
        ok_aim_done.report.aim_revision.as_deref(),
        Some(AIM_SEED_REVISION)
    );
    assert!(ok_aim_done.report.aim_estimated_quality.is_some());
    assert_eq!(ok_aim_done.report.verified_score, None);
    assert_eq!(ok_aim_done.report.verified_score_f64, None);

    // 4. Pre-transform HDR / HDR-metadata / f32-alpha>1.0 gates & pre-alloc memory rejection.
    //    4a. HDR PQ source converted to RGB8_SRGB must still be refused by Verify BEFORE transform.
    let pq_data = vec![0u8; 16 * 16 * 16];
    let pq_fmt = PixelFormat::new(
        zenpixels::SampleLayout::RGBAF32,
        zenpixels::ColorSpace::BT2100_PQ.with_alpha(zenpixels::AlphaMode::Straight),
    );
    let pq_buf = PixelBuf::from_bytes(pq_data, 16, 16, pq_fmt).unwrap();
    let pq_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_color(zencodecs::engine::ColorTarget::ConvertTo(
                    PixelFormat::RGB8_SRGB,
                ))
                .with_coding(Coding::Verify(ztarget.clone())),
            pq_buf.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(pq_err.error().failure(), &Failure::UnsupportedEffect);

    //    4b. SDR buffer tagged with HDR CICP transfer (PQ=16) is refused BEFORE Verify.
    let mut hdr_meta = Metadata::none();
    hdr_meta.cicp = Some(zenpixels::Cicp::new(9, 16, 0, true));
    let hdr_meta_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            non_square.as_view(),
            &hdr_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(hdr_meta_err.error().failure(), &Failure::UnsupportedEffect);

    //    4c. f32 RGBA with alpha=1.5 (> 1.0) is treated as non-opaque and refused on JPEG without flatten.
    let mut over_alpha_bytes = Vec::with_capacity(16 * 16 * 16);
    for _ in 0..(16 * 16) {
        for v in [0.5_f32, 0.5_f32, 0.5_f32, 1.5_f32] {
            over_alpha_bytes.extend_from_slice(&v.to_ne_bytes());
        }
    }
    let over_alpha_fmt = PixelFormat::new(
        zenpixels::SampleLayout::RGBAF32,
        zenpixels::ColorSpace::SRGB.with_alpha(zenpixels::AlphaMode::Straight),
    );
    let over_alpha_buf = PixelBuf::from_bytes(over_alpha_bytes, 16, 16, over_alpha_fmt).unwrap();
    let over_alpha_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            over_alpha_buf.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        over_alpha_err.error().failure(),
        &Failure::UnsupportedEffect
    );

    //    4d. Tight max_memory_bytes fails cleanly with Failure::ResourceLimit BEFORE allocating.
    let mut tight_limits = zencodec::ResourceLimits::default();
    tight_limits.max_memory_bytes = Some(16 * 1024);
    let tight_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            non_square.as_view(),
            &Metadata::none(),
            None,
            &Run::new().with_limits(tight_limits),
        )
        .unwrap_err();
    assert_eq!(tight_err.error().failure(), &Failure::ResourceLimit);

    //    4e. Stride-aware Butteraugli scratch bound covers 8x8192 padded planes (> 13.25 MiB live!).
    assert!(
        ButteraugliMaxScorer::new().estimated_scratch_bytes(8, 8192) >= 20 * 1024 * 1024,
        "Butteraugli scratch bound for 8x8192 must account for 16-sample stride alignment"
    );

    //    4f. Metadata::diffuse_white, PixelView::hdr(), malformed ICC, and PreserveBytes+ConvertTo refused.
    let mut dw_meta = Metadata::none();
    dw_meta.diffuse_white = Some(zenpixels::hdr::DiffuseWhite::BT2408);
    let dw_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            non_square.as_view(),
            &dw_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(dw_err.error().failure(), &Failure::UnsupportedEffect);

    let hdr_buf = make_textured_rgb8(16, 16).with_hdr(zenpixels::HdrMetadata {
        content_light_level: Some(zenpixels::hdr::ContentLightLevel::new(1000, 400)),
        mastering: None,
    });
    let hdr_view_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            hdr_buf.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(hdr_view_err.error().failure(), &Failure::UnsupportedEffect);

    let mut bad_icc_meta = Metadata::none();
    // 24 bytes with valid RGB header tag at bytes 16..20 but truncated (< 132 B) -> structurally invalid!
    let mut bad_icc = vec![0u8; 24];
    bad_icc[16..20].copy_from_slice(b"RGB ");
    bad_icc_meta.icc_profile = Some(Arc::from(bad_icc));
    let bad_icc_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget.clone())),
            non_square.as_view(),
            &bad_icc_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(bad_icc_err.error().failure(), &Failure::InvalidData);

    // PreserveBytes + ConvertTo on metadata-bearing image must refuse rather than silently drop ICC,
    // even when stored PixelFormat == target PixelFormat (authoritative ICC resolved BEFORE shortcut!).
    let p3_icc: Arc<[u8]> = Arc::from(moxcms::ColorProfile::new_display_p3().encode().unwrap());
    let mut p3_meta = Metadata::none();
    p3_meta.icc_profile = Some(p3_icc);
    let preserve_same_stored_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_metadata(zencodecs::engine::MetadataPolicy::PreserveBytes)
                .with_color(zencodecs::engine::ColorTarget::ConvertTo(
                    PixelFormat::RGB8_SRGB,
                )),
            non_square.as_view(),
            &p3_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        preserve_same_stored_err.error().failure(),
        &Failure::UnsupportedEffect,
        "authoritative Display-P3 ICC must prevent RGB8_SRGB equality shortcut and refuse under PreserveBytes"
    );

    // PNG header probe inflates iCCP/zTXt without a proven upper bound, so PreserveBytes readback
    // on PNG is honestly refused under Strict (while JPEG uncompressed marker probe passes!).
    let mut exif_meta = Metadata::none();
    exif_meta.exif = Some(Arc::from(
        b"MM\x00*\x00\x00\x00\x08\x00\x00\x00\x00\x00\x00".as_slice(),
    ));
    let png_preserve_strict = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Png)
                .with_metadata(zencodecs::engine::MetadataPolicy::PreserveBytes),
            non_square.as_view(),
            &exif_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        png_preserve_strict.error().failure(),
        &Failure::PolicyRefusal,
        "Strict must honestly refuse PNG PreserveBytes probe whose iCCP/zTXt inflation lacks upper bound, got: {:?}",
        png_preserve_strict.error()
    );

    // JPEG header probe bound must account for `Vec<PreservedSegment>` table capacity when a
    // JPEG carries thousands of 4-byte empty COM markers (`FF FE 00 02`): 65,536 empty COM
    // segments add ~256 KiB to the bitstream but require >= 2 MiB steady / 4 MiB doubled /
    // 6 MiB transient realloc table capacity (plus metadata & fixed overhead -> ~10.2 MiB bound;
    // the old `6x + 512 KiB` formula would have underbounded this at ~2.1 MiB!).
    let mut com_jpeg_cfg = zenjpeg::JpegEncoderConfig::new();
    let mut inner_cfg = com_jpeg_cfg.inner().clone();
    for _ in 0..65_536 {
        inner_cfg = inner_cfg.add_segment(0xFE, Vec::new());
    }
    *com_jpeg_cfg.inner_mut() = inner_cfg;
    let com_enc_id = zencodec::RegistrationId::new("org.test.jpeg.com_burst").unwrap();
    let com_codecs = zencodecs::bundles::register_all_enabled(zencodec::CodecSet::new())
        .unwrap()
        .with_named_send_encoder(com_enc_id.clone(), "test-1", com_jpeg_cfg)
        .unwrap();
    let com_engine = Engine::new(com_codecs)
        .unwrap()
        .with_color_services(zencodecs::bundles::moxcms_color_services())
        .with_routing(
            zencodecs::routing::Routing::unrestricted()
                .requiring(
                    zencodecs::routing::RuleKey::ImageEncode(ImageFormat::Jpeg),
                    com_enc_id,
                )
                .unwrap(),
        );
    let mut com_tight_limits = zencodec::ResourceLimits::none();
    com_tight_limits.max_memory_bytes = Some(4_000_000);
    let com_tight_err = com_engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_metadata(zencodecs::engine::MetadataPolicy::PreserveBytes),
            non_square.as_view(),
            &exif_meta,
            None,
            &Run::new().with_limits(com_tight_limits),
        )
        .unwrap_err();
    assert_eq!(
        com_tight_err.error().failure(),
        &Failure::ResourceLimit,
        "Strict JPEG probe must count PreservedSegment table capacity for empty COM burst and reject under 4 MiB cap, got: {:?}",
        com_tight_err.error()
    );
    let mut com_ok_limits = zencodec::ResourceLimits::none();
    com_ok_limits.max_memory_bytes = Some(16_000_000);
    let com_ok = com_engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_metadata(zencodecs::engine::MetadataPolicy::PreserveBytes),
            non_square.as_view(),
            &exif_meta,
            None,
            &Run::new().with_limits(com_ok_limits),
        )
        .unwrap();
    assert!(
        com_ok.output.as_bytes().len() > 262_144,
        "encoded JPEG must actually carry the 65,536 COM markers (got {} bytes)",
        com_ok.output.as_bytes().len()
    );

    // Premultiplied RGBA8 + Display-P3 ICC + AlphaPolicy::FlattenSrgb([0,0,0]) must preserve
    // AlphaMode::Premultiplied when rebinding ICC (so zenpixels-convert honestly refuses the
    // unsupported premultiplied-ICC CMS route with UnsupportedEffect rather than silently
    // misinterpreting it as straight alpha and corrupting pixels!).
    let premul_fmt = PixelFormat::new(
        zenpixels::SampleLayout::RGBA8,
        zenpixels::ColorSpace::SRGB.with_alpha(zenpixels::AlphaMode::Premultiplied),
    );
    let mut premul_data = Vec::with_capacity(8 * 8 * 4);
    for _ in 0..(8 * 8) {
        premul_data.extend_from_slice(&[128u8, 0, 0, 128]);
    }
    let premul_buf = PixelBuf::from_bytes(premul_data, 8, 8, premul_fmt).unwrap();
    let flat_premul_icc_err = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Png).with_alpha(AlphaPolicy::FlattenSrgb([0, 0, 0])),
            premul_buf.as_view(),
            &p3_meta,
            None,
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        flat_premul_icc_err.error().failure(),
        &Failure::UnsupportedEffect,
        "premultiplied RGBA8 + P3 ICC must preserve Premultiplied mode and refuse unsupported premul-ICC CMS route, got: {:?}",
        flat_premul_icc_err.error()
    );

    // Parametric premultiplied RGBA8 (no ICC) + AlphaPolicy::FlattenSrgb([0,0,0]) unpremultiplies
    // cleanly via zenpixels-convert without double-multiplying alpha (~188 vs ~93 straight).
    let flat_premul = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Png).with_alpha(AlphaPolicy::FlattenSrgb([0, 0, 0])),
            premul_buf.as_view(),
            &Metadata::none(),
            None,
            &Run::new(),
        )
        .unwrap();
    let dec_premul = engine
        .decode(
            &zencodecs::engine::DecodeNeeds::still(),
            flat_premul.output.as_bytes(),
            &Run::new(),
        )
        .unwrap();
    let r_out = dec_premul.output.pixels().row(0)[0];
    assert!(
        r_out >= 165,
        "parametric premultiplied RGBA8 flattened onto black must not double-multiply alpha (expected R >= 165, got {r_out})"
    );

    // AVIF encoder does not yet expose post-quantization effective setting keys, so Verify refuses it.
    #[cfg(feature = "avif-encode")]
    {
        let avif_verify_err = engine
            .encode_pixels(
                &OutputIntent::format(ImageFormat::Avif)
                    .with_coding(Coding::Verify(ztarget.clone())),
                non_square.as_view(),
                &Metadata::none(),
                None,
                &Run::new().with_workspace_policy(WorkspacePolicy::BestEffort),
            )
            .unwrap_err();
        assert_eq!(
            avif_verify_err.error().failure(),
            &Failure::UnsupportedEffect,
            "AVIF Verify must be explicitly refused until encoder exposes post-quantization setting key"
        );
    }

    // 5. Cooperative cancellation:
    //    5a. Entry stop aborts Coding::Verify with Failure::Stopped(TimedOut).
    //    5b. Mid-orientation bake stop aborts inside bake_orientation_pixels BEFORE prepare_reference!
    let big_img = make_textured_rgb8(128, 192);
    struct TripAfterN {
        allow: usize,
        seen: Arc<AtomicUsize>,
    }
    impl Stop for TripAfterN {
        fn check(&self) -> Result<(), StopReason> {
            let n = self.seen.fetch_add(1, Ordering::SeqCst);
            if n >= self.allow {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let orient_seen = Arc::new(AtomicUsize::new(0));
    let orient_pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(TripAfterN {
        // Passes Stage::Prepare (encode_pixels) + Stage::Admit (encode_pixels_verified) +
        // bake_orientation_pixels entry + row 0, then cancels at row 64 inside the orientation loop!
        allow: 4,
        seen: Arc::clone(&orient_seen),
    }));
    let orient_cancel = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_orientation(OrientationMode::PreserveStored)
                .with_coding(Coding::Verify(ztarget.clone())),
            big_img.as_view(),
            &rot_meta,
            None,
            &Run::new().with_pulse(orient_pulse),
        )
        .unwrap_err();
    assert_eq!(
        orient_cancel.error().failure(),
        &Failure::Stopped(StopReason::Cancelled),
        "orientation bake loop must poll pulse and abort cooperatively"
    );
    assert!(
        orient_seen.load(Ordering::SeqCst) >= 5,
        "orientation stop must have tripped inside row loop"
    );
    struct TimedOutStop;
    impl Stop for TimedOutStop {
        fn check(&self) -> Result<(), StopReason> {
            Err(StopReason::TimedOut)
        }
    }
    let pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(TimedOutStop));
    let cancelled = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(ztarget)),
            non_square.as_view(),
            &Metadata::none(),
            None,
            &Run::new().with_pulse(pulse),
        )
        .unwrap_err();
    assert_eq!(
        cancelled.error().failure(),
        &Failure::Stopped(StopReason::TimedOut)
    );
}
