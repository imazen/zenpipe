//! Resource estimation and encode budgeting.
//!
//! Predicts encode/decode peak memory and wall-time **without running**, by
//! building the same per-format config the real encode would use and querying
//! the codec's `estimate_{encode,decode}_resources`. A codec without a
//! calibrated cost model returns [`ResourceEstimate::unknown`] (all-`None`), so
//! today the exact frame buffer is the certain term and the wall-time-driven
//! effort loop degrades to a graceful no-op until cost models ship — the same
//! posture imageflow's memory gate uses.

use crate::config::CodecConfig;
use crate::quality::QualityIntent;
use crate::{CodecError, Limits, Result};
use zencodec::ImageFormat;

pub use zencodec::estimate::{ComputeEnvironment, ImageCharacteristics, ResourceEstimate};

/// Bytes of one decoded BGRA8/RGBA8 working frame for `image` — the exact,
/// effort-independent memory term that dominates until codecs model their
/// working set. Four bytes/pixel: the pipeline intermediate, independent of the
/// source descriptor.
pub(crate) fn frame_buffer_bytes(image: &ImageCharacteristics) -> u64 {
    image.pixels().saturating_mul(4)
}

/// Estimate encode resources for `format` at `quality` **without encoding**.
///
/// Builds the exact per-format encoder config that [`crate::dispatch`] would, so
/// the estimate is self-consistent with the real encode, then calls the codec's
/// `estimate_encode_resources`. Errors only on an unsupported/disabled output
/// format. The returned estimate is `unknown()` for codecs that don't model
/// their resource use — see the module docs.
pub fn estimate_encode(
    format: ImageFormat,
    quality: &QualityIntent,
    codec_config: Option<&CodecConfig>,
    image: &ImageCharacteristics,
    compute: &ComputeEnvironment,
) -> Result<ResourceEstimate> {
    use zencodec::encode::EncoderConfig;
    let q = Some(quality.quality);
    let e = quality.effort;
    // Used by every per-format arm; bound here so a decode-only feature set
    // (no encoders compiled) doesn't warn on unused params.
    let _ = (q, e, codec_config, image, compute);
    let est = match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => crate::codecs::jpeg::build_encoding(q, codec_config)
            .estimate_encode_resources(image, compute),
        #[cfg(feature = "webp")]
        ImageFormat::WebP => {
            crate::codecs::webp::build_encoding(q, e, quality.lossless, codec_config)
                .estimate_encode_resources(image, compute)
        }
        #[cfg(feature = "png")]
        ImageFormat::Png => {
            crate::codecs::png::build_encoding(q, e, quality.lossless, codec_config, None)
                .estimate_encode_resources(image, compute)
        }
        #[cfg(feature = "avif-encode")]
        ImageFormat::Avif => crate::codecs::avif_enc::build_encoding(q, e, codec_config)
            .estimate_encode_resources(image, compute),
        #[cfg(feature = "jxl-encode")]
        ImageFormat::Jxl => crate::codecs::jxl_enc::build_encoding(q, e, codec_config)
            .estimate_encode_resources(image, compute),
        #[cfg(feature = "gif")]
        ImageFormat::Gif => crate::codecs::gif::build_gif_encoding(codec_config)
            .estimate_encode_resources(image, compute),
        _ => return Err(whereat::at!(CodecError::UnsupportedFormat(format))),
    };
    Ok(est)
}

/// Estimate decode resources for `format` **without decoding**, using a default
/// per-format decoder config. Advisory: returns [`ResourceEstimate::unknown`]
/// for a format whose decoder isn't compiled in, and codecs that don't model
/// decode cost return `unknown` too (the frame buffer then dominates).
pub fn estimate_decode(
    format: ImageFormat,
    image: &ImageCharacteristics,
    compute: &ComputeEnvironment,
) -> ResourceEstimate {
    use zencodec::decode::DecoderConfig;
    let _ = (image, compute);
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => {
            zenjpeg::JpegDecoderConfig::default().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "webp")]
        ImageFormat::WebP => zenwebp::zencodec::WebpDecoderConfig::default()
            .estimate_decode_resources(image, compute),
        #[cfg(feature = "png")]
        ImageFormat::Png => {
            zenpng::PngDecoderConfig::default().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "jxl-decode")]
        ImageFormat::Jxl => {
            zenjxl::JxlDecoderConfig::default().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "gif")]
        ImageFormat::Gif => {
            zengif::GifDecoderConfig::default().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => {
            zenavif::AvifDecoderConfig::new().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => {
            heic::HeicDecoderConfig::new().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "bitmaps-bmp")]
        ImageFormat::Bmp => {
            zenbitmaps::BmpDecoderConfig::new().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "pdf-decode")]
        ImageFormat::Pdf => {
            zenpdf::PdfDecoderConfig::new().estimate_decode_resources(image, compute)
        }
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
            zenraw::RawDecoderConfig::new().estimate_decode_resources(image, compute)
        }
        // Tiff is a stub feature (zentiff not wired into zencodecs yet, zenpipe#43) —
        // it falls through to unknown() until that integration lands.
        _ => ResourceEstimate::unknown(),
    }
}

/// Peak job memory, mirroring imageflow's gate:
/// `max(decode_peak, encode_peak) + one frame buffer`. Returns
/// `(expected_avg, conservative_max)` bytes. Codec working sets are 0 until
/// codecs model them, so the buffer dominates today.
pub fn peak_job_bytes(
    decode: &ResourceEstimate,
    encode: &ResourceEstimate,
    image: &ImageCharacteristics,
) -> (u64, u64) {
    let buffer = frame_buffer_bytes(image);
    let d_avg = decode.peak_memory_bytes_est().unwrap_or(0);
    let d_max = decode.peak_memory_bytes_max().unwrap_or(d_avg);
    let e_avg = encode.peak_memory_bytes_est().unwrap_or(0);
    let e_max = encode.peak_memory_bytes_max().unwrap_or(e_avg);
    (
        d_avg.max(e_avg).saturating_add(buffer),
        d_max.max(e_max).saturating_add(buffer),
    )
}

/// Reject before running if the estimated peak memory for `estimate` over
/// `image` exceeds `limits.max_memory_bytes`. Uses the codec working-set
/// estimate (0 until modeled) plus the exact frame buffer. Other `Limits` fields
/// are unaffected; a no-op when `max_memory_bytes` is unset.
pub fn check_estimate_against_limits(
    estimate: &ResourceEstimate,
    image: &ImageCharacteristics,
    limits: &Limits,
) -> Result<()> {
    if let Some(max) = limits.max_memory_bytes {
        let buffer = frame_buffer_bytes(image);
        let codec = estimate
            .peak_memory_bytes_max()
            .or_else(|| estimate.peak_memory_bytes_est())
            .unwrap_or(0);
        let peak = codec.saturating_add(buffer);
        if peak > max {
            return Err(whereat::at!(CodecError::LimitExceeded(alloc::format!(
                "estimated peak memory {peak} bytes exceeds max_memory_bytes {max}"
            ))));
        }
    }
    Ok(())
}

/// A soft resource budget for one encode. Absent fields are unconstrained.
///
/// The effort loop ([`plan_encode_effort`]) fits an encode under these by
/// trading compression effort for speed; it is advisory for wall-time until
/// codecs ship time estimators, while the memory term is enforceable today via
/// the exact frame buffer.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct EncodeBudget {
    /// Max wall-clock encode time (ms) at `cores`. Evaluated only when the codec
    /// models wall time; otherwise the loop leaves effort untouched.
    pub wall_ms: Option<u64>,
    /// Cores available for the encode (folds into the estimate via `at_cores`).
    pub cores: Option<u32>,
    /// Max peak memory (bytes): codec working set + the exact frame buffer.
    pub peak_mem_bytes: Option<u64>,
}

/// Outcome of fitting an effort to an [`EncodeBudget`].
#[derive(Clone, Debug)]
pub struct EffortPlan {
    /// Chosen generic effort. `None` keeps the codec/quality default (budget not
    /// binding, or not evaluable); `Some(n)` is a reduced effort forced to fit.
    pub effort: Option<u32>,
    /// The estimate at the chosen effort.
    pub estimate: ResourceEstimate,
    /// Whether the chosen effort has no KNOWN budget violation. `false` only when
    /// even the lowest effort exceeds an evaluable budget.
    pub fits: bool,
}

/// Generic effort levels for the budget sweep, highest (best compression) first.
/// Each codec clamps to its native range; "higher = slower, better compression".
const EFFORT_SWEEP: [u32; 9] = [9, 8, 7, 6, 5, 4, 3, 2, 1];

enum BudgetFit {
    Fits,
    Exceeds,
    Unevaluable,
}

fn estimate_fits(
    est: &ResourceEstimate,
    image: &ImageCharacteristics,
    budget: &EncodeBudget,
) -> BudgetFit {
    let mut evaluated = false;
    if let Some(limit) = budget.wall_ms
        && let Some(ms) = est.wall_ms()
    {
        evaluated = true;
        if ms > limit {
            return BudgetFit::Exceeds;
        }
    }
    if let Some(limit) = budget.peak_mem_bytes {
        // The frame buffer is exact + effort-independent; the codec working set
        // is added when modeled. Memory is thus evaluable today.
        evaluated = true;
        let buffer = frame_buffer_bytes(image);
        let codec = est
            .peak_memory_bytes_max()
            .or_else(|| est.peak_memory_bytes_est())
            .unwrap_or(0);
        if codec.saturating_add(buffer) > limit {
            return BudgetFit::Exceeds;
        }
    }
    if evaluated {
        BudgetFit::Fits
    } else {
        BudgetFit::Unevaluable
    }
}

/// Pick the highest generic effort whose estimate fits `budget` for encoding
/// `format` at `quality`.
///
/// Keeps the quality's own effort when the budget isn't binding at the default
/// effort, or can't be evaluated (e.g. a wall-time budget against a codec that
/// doesn't model time); only reduces effort under real pressure. Returns
/// `fits = false` only when even the lowest effort exceeds an evaluable budget
/// (e.g. the frame buffer alone exceeds a memory budget).
pub fn plan_encode_effort(
    format: ImageFormat,
    quality: &QualityIntent,
    codec_config: Option<&CodecConfig>,
    image: &ImageCharacteristics,
    budget: &EncodeBudget,
) -> Result<EffortPlan> {
    let compute = ComputeEnvironment::new().with_cores(budget.cores.unwrap_or(1).max(1) as usize);

    // 1) Estimate at the quality's own effort (codec default when None).
    let base = estimate_encode(format, quality, codec_config, image, &compute)?;
    match estimate_fits(&base, image, budget) {
        // Budget not binding, or no estimator to evaluate it → don't override.
        BudgetFit::Fits | BudgetFit::Unevaluable => {
            return Ok(EffortPlan {
                effort: quality.effort,
                estimate: base,
                fits: true,
            });
        }
        BudgetFit::Exceeds => {}
    }

    // 2) Default effort exceeds an evaluable budget → reduce effort to fit.
    //    Sweep high→low; the first that fits is the highest-effort fit (best
    //    compression within budget). Efforts >= the default also exceed and are
    //    simply skipped.
    for &effort in &EFFORT_SWEEP {
        let q = quality.clone().with_effort(effort);
        let est = estimate_encode(format, &q, codec_config, image, &compute)?;
        if matches!(estimate_fits(&est, image, budget), BudgetFit::Fits) {
            return Ok(EffortPlan {
                effort: Some(effort),
                estimate: est,
                fits: true,
            });
        }
    }

    // 3) Nothing fits even at the lowest effort. Report it, not fitting.
    let lowest = *EFFORT_SWEEP.last().unwrap();
    let q = quality.clone().with_effort(lowest);
    let est = estimate_encode(format, &q, codec_config, image, &compute)?;
    Ok(EffortPlan {
        effort: Some(lowest),
        estimate: est,
        fits: false,
    })
}

#[cfg(test)]
#[cfg(feature = "webp")] // a compiled lossy encoder with an effort dial
mod tests {
    use super::*;

    fn chars(w: u32, h: u32) -> ImageCharacteristics {
        ImageCharacteristics::new(w, h, zenpixels::PixelDescriptor::RGBA8_SRGB)
    }

    #[test]
    fn encode_decode_estimate_buffer_dominates_until_models_ship() {
        let q = QualityIntent::from_quality(75.0);
        let c = chars(1000, 1000);
        let env = ComputeEnvironment::new().with_cores(4);
        let enc = estimate_encode(ImageFormat::WebP, &q, None, &c, &env).unwrap();
        let dec = estimate_decode(ImageFormat::WebP, &c, &env);
        let (avg, max) = peak_job_bytes(&dec, &enc, &c);
        let buffer = 1000u64 * 1000 * 4;
        assert!(avg >= buffer && max >= buffer);
        // No codec models its working set yet → peak == the exact buffer.
        if enc.peak_memory_bytes_est().is_none() && dec.peak_memory_bytes_est().is_none() {
            assert_eq!(avg, buffer);
        }
    }

    #[test]
    fn unsupported_encode_format_errors() {
        let c = chars(64, 64);
        let env = ComputeEnvironment::new();
        // Pnm has no encoder wired → error.
        assert!(
            estimate_encode(
                ImageFormat::Pnm,
                &QualityIntent::from_quality(75.0),
                None,
                &c,
                &env
            )
            .is_err()
        );
    }

    #[test]
    fn limits_gate_rejects_when_buffer_exceeds_else_passes() {
        let c = chars(2000, 2000); // 16 MB frame buffer
        let env = ComputeEnvironment::new();
        let enc = estimate_encode(
            ImageFormat::WebP,
            &QualityIntent::from_quality(75.0),
            None,
            &c,
            &env,
        )
        .unwrap();
        let tight = Limits {
            max_memory_bytes: Some(1_000_000),
            ..Limits::none()
        };
        assert!(check_estimate_against_limits(&enc, &c, &tight).is_err());
        let loose = Limits {
            max_memory_bytes: Some(64_000_000),
            ..Limits::none()
        };
        assert!(check_estimate_against_limits(&enc, &c, &loose).is_ok());
    }

    #[test]
    fn effort_loop_keeps_default_when_unconstrained() {
        let q = QualityIntent::from_quality(75.0);
        let c = chars(500, 500);
        let plan =
            plan_encode_effort(ImageFormat::WebP, &q, None, &c, &EncodeBudget::default()).unwrap();
        assert_eq!(plan.effort, q.effort); // unchanged (None)
        assert!(plan.fits);
    }

    #[test]
    fn effort_loop_generous_memory_budget_keeps_default() {
        let q = QualityIntent::from_quality(75.0);
        let c = chars(500, 500); // 1 MB buffer
        let budget = EncodeBudget {
            peak_mem_bytes: Some(64_000_000),
            ..Default::default()
        };
        let plan = plan_encode_effort(ImageFormat::WebP, &q, None, &c, &budget).unwrap();
        assert_eq!(plan.effort, None); // default kept (buffer fits)
        assert!(plan.fits);
    }

    #[test]
    fn effort_loop_unmeetable_memory_budget_reports_unfit() {
        let q = QualityIntent::from_quality(75.0);
        let c = chars(2000, 2000); // 16 MB buffer
        // Below the effort-independent frame buffer → no effort can fit.
        let budget = EncodeBudget {
            peak_mem_bytes: Some(1_000_000),
            ..Default::default()
        };
        let plan = plan_encode_effort(ImageFormat::WebP, &q, None, &c, &budget).unwrap();
        assert!(!plan.fits);
    }
}
