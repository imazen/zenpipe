//! AVIF backend + knob auto-tuning for the pipeline's encode step.
//!
//! zenpipe is the **consumer** here. All of the tuning logic — the bake
//! contract, the measured default table, the routing rules — lives in
//! [`zenavif::backend_tuner`], because a codec owns its own tuning code.
//! This module is the ~100 lines that connect a pipeline's
//! "encode to about this quality, within about this long" intent to that
//! decision, and hand back a config to encode with.
//!
//! Gated on the off-by-default `avif-autotune` feature.
//!
//! # Why the dependency is spelled `zenavif_tuner`
//!
//! zenpipe and `zencodecs` depend on zenavif **0.1.x** for the ordinary
//! decode/encode path. The backend tuner is 0.2.x. Rather than migrate
//! that whole edge — a change with nothing additive about it — this
//! feature takes the newer crate as a separately-named direct dependency
//! (`zenavif_tuner`, package `zenavif`, sibling path `../zenavif`). The
//! two coexist because they are semver-incompatible, and nothing here
//! hands a 0.2 type to a 0.1 API: this module produces a config and
//! encodes with it through the same crate it came from.
//!
//! It is a **path** dep, not a git one, because zenavif's `auto-tune`
//! feature path-pins its own zenanalyze/zenpredict deps to
//! `../zenanalyze` — which a git dep cannot resolve. Same sibling-checkout
//! convention zenavif itself uses.
//!
//! When the ordinary edge moves to 0.2.x, delete the rename and the
//! second dependency; nothing else in this module changes.
//!
//! # Usage
//!
//! ```ignore
//! use zenpipe::avif_autotune::{AvifAutotune, AvifIntent};
//!
//! // Once, at startup. Swap `stub()` for `from_bake(&bytes)` when the
//! // trained bake lands — the rest of this snippet is unchanged.
//! let tuner = AvifAutotune::stub();
//!
//! let plan = tuner.plan(&rgb8, width, height, AvifIntent::new(82.0).within_ms(250.0))?;
//! println!("routing to {:?} ({})", plan.backend(), plan.explain());
//! let avif = tuner.encode(&rgb8, width, height, &plan)?;
//! ```
//!
//! # What it does not do
//!
//! It does **not** flip any default. Nothing in the ordinary
//! `CodecIntent` → `zencodecs` encode path consults this module; a
//! caller opts in by holding an [`AvifAutotune`] and calling it.
//!
//! `imageflow` is **not** wired to this — out of scope. zenpipe is the
//! intended forward backend for imageflow v3 but is not wired into it
//! today, so this lands on the zenpipe side only.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use zenavif_tuner::backend_tuner::{AllowedBackends, AvifTuning, StubTuner, TuneRequest};
use zenavif_tuner::{Av1Backend, AvifTune, AvifTuner, QualityTarget};

/// What the pipeline wants from an AVIF encode.
///
/// The size is not part of this type — it comes from the frame being
/// encoded, and it is load-bearing for the time estimate (`alpha + beta
/// * megapixels`; a bare ms/MP misprices small images by ~20x).
#[derive(Debug, Clone)]
pub struct AvifIntent {
    target_quality: f32,
    time_budget_ms: Option<f32>,
    allowed_backends: Option<AllowedBackends>,
    has_alpha: bool,
}

impl AvifIntent {
    /// Encode to about `target_quality`, in the tuner's metric space
    /// (zensim Zq for the bakes this is built for), with no time budget.
    pub fn new(target_quality: f32) -> Self {
        Self {
            target_quality,
            time_budget_ms: None,
            allowed_backends: None,
            has_alpha: false,
        }
    }

    /// Prefer a backend whose predicted wall time fits `ms`.
    ///
    /// A **preference**, not a deadline: the underlying estimate is a
    /// pooled median that the instrument itself measured as wrong by up
    /// to 24x per image, and it is single-threaded q45 wall time on one
    /// host. Use it to route, not to promise.
    #[must_use]
    pub fn within_ms(mut self, ms: f32) -> Self {
        self.time_budget_ms = Some(ms);
        self
    }

    /// Restrict which AV1 backends may be chosen. Defaults to every
    /// backend the zenavif build can encode with.
    #[must_use]
    pub fn allowing(mut self, allowed: AllowedBackends) -> Self {
        self.allowed_backends = Some(allowed);
        self
    }

    /// Declare that the frame carries alpha (masks out backends whose
    /// still seam refuses it).
    #[must_use]
    pub fn with_alpha(mut self, has_alpha: bool) -> Self {
        self.has_alpha = has_alpha;
        self
    }

    fn to_request(&self, width: u32, height: u32) -> TuneRequest {
        let mut req = TuneRequest::new(
            QualityTarget::Zensim(self.target_quality),
            width,
            height,
        )
        .with_alpha(self.has_alpha);
        if let Some(ms) = self.time_budget_ms {
            req = req.with_time_budget_ms(ms);
        }
        if let Some(allowed) = self.allowed_backends {
            req = req.with_allowed_backends(allowed);
        }
        req
    }
}

/// The tuner, held by the consumer for the life of the process.
///
/// Two constructors, and [`AvifPlan::explain`] always says which one
/// answered — a pipeline that logs its decisions should log that, so a
/// "the tuner chose X" line can never be mistaken for a model prediction
/// when no model is loaded.
pub struct AvifAutotune {
    inner: Inner,
}

enum Inner {
    Stub(StubTuner),
    Model(AvifTuner),
}

impl AvifAutotune {
    /// The measured-default tuner — no model file.
    ///
    /// Integrate against this now; swap to [`from_bake`](Self::from_bake)
    /// when the trained bake exists. Nothing else in the call site
    /// changes.
    pub fn stub() -> Self {
        Self {
            inner: Inner::Stub(StubTuner::new()),
        }
    }

    /// Load a ZNPR v3 backend-tuner bake.
    ///
    /// The bytes are the **caller's** — zenavif bundles no weights, so
    /// the pipeline decides where its model comes from (a file, an
    /// embedded asset, a download). The contract the bake must declare
    /// is in zenavif's `docs/AUTOTUNE_CONTRACT.md`.
    ///
    /// # Errors
    ///
    /// The bake is not parseable ZNPR, or does not declare a well-formed
    /// tune contract. Both fail loudly rather than falling back to the
    /// stub: a pipeline that asked for a model and silently got defaults
    /// would report predictions it never made.
    pub fn from_bake(bytes: &[u8]) -> Result<Self, AvifAutotuneError> {
        AvifTuner::from_bytes(bytes)
            .map(|t| Self {
                inner: Inner::Model(t),
            })
            .map_err(|e| AvifAutotuneError::Bake(e.to_string()))
    }

    /// Choose a backend and knobs for one frame.
    ///
    /// `rgb` is packed RGB8, `width * height * 3` bytes. The model path
    /// analyzes it (or reuses a shared zenanalyze pass if one is
    /// threaded through in a later revision); the stub does not read it.
    ///
    /// # Errors
    ///
    /// Every backend was masked out — by the caller's mask, by alpha, or
    /// by a time budget nothing can meet. That is a refusal, not a
    /// fallback: handing back a config that blows the stated budget would
    /// be worse than saying no.
    pub fn plan(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
        intent: AvifIntent,
    ) -> Result<AvifPlan, AvifAutotuneError> {
        let req = intent.to_request(width, height);
        let tuned = match &self.inner {
            Inner::Stub(t) => t.tune(rgb, None, &req),
            Inner::Model(t) => t.tune(rgb, None, &req),
        }
        .map_err(|e| AvifAutotuneError::Tune(e.to_string()))?;
        Ok(AvifPlan { tuned })
    }

    /// Encode `rgb` with a plan's config.
    ///
    /// A convenience over `zenavif::encode_rgb8` so a caller does not
    /// have to reach for the `imgref` and `StopToken` shapes just to use
    /// the plan.
    ///
    /// # Errors
    ///
    /// Whatever the zenavif encoder reports, with its message preserved.
    pub fn encode(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
        plan: &AvifPlan,
    ) -> Result<Vec<u8>, AvifAutotuneError> {
        let expected = (width as usize) * (height as usize) * 3;
        if rgb.len() != expected {
            return Err(AvifAutotuneError::Tune(alloc::format!(
                "RGB8 buffer is {} bytes, expected {expected} for {width}x{height}",
                rgb.len()
            )));
        }
        let px: Vec<rgb::Rgb<u8>> = rgb
            .chunks_exact(3)
            .map(|c| rgb::Rgb::new(c[0], c[1], c[2]))
            .collect();
        let img = imgref::Img::new(px, width as usize, height as usize);
        zenavif_tuner::encode_rgb8(
            img.as_ref(),
            plan.tuned.config(),
            almost_enough::StopToken::new(almost_enough::Unstoppable),
        )
        .map(|e| e.avif_file)
        .map_err(|e| AvifAutotuneError::Encode(e.to_string()))
    }
}

/// One frame's tuning decision.
pub struct AvifPlan {
    tuned: AvifTune,
}

impl AvifPlan {
    /// The chosen AV1 backend.
    pub fn backend(&self) -> Av1Backend {
        self.tuned.backend()
    }

    /// Predicted encoded size in bytes, when a model with a `bytes_log`
    /// head made the call. `None` from the stub, which predicts nothing.
    pub fn expected_bytes(&self) -> Option<f32> {
        self.tuned.expected_bytes()
    }

    /// Predicted wall time in milliseconds, or `None` when the
    /// (backend, speed) pair has no measured fit and the bake carries no
    /// time head. `None` means NOT MEASURED — never "fast".
    pub fn expected_wall_ms(&self) -> Option<f32> {
        self.tuned.expected_wall_ms()
    }

    /// A log line: the cell, where the decision came from, and what it
    /// costs. Written for a pipeline trace, and deliberately explicit
    /// about `stub` vs `model` so the two are never confused in a log.
    pub fn explain(&self) -> String {
        let source = match self.tuned.source() {
            zenavif_tuner::TuneSource::Model => "model",
            zenavif_tuner::TuneSource::Stub => "stub (measured defaults, no model)",
            _ => "unknown",
        };
        let ms = match self.tuned.expected_wall_ms() {
            Some(v) => alloc::format!("{v:.0} ms"),
            None => "NOT MEASURED".to_string(),
        };
        let bytes = match self.tuned.expected_bytes() {
            Some(v) => alloc::format!("{v:.0} B"),
            None => "not predicted".to_string(),
        };
        alloc::format!(
            "cell={} source={source} expected_wall={ms} expected_bytes={bytes}",
            self.tuned.cell_label()
        )
    }

    /// The encoder config to use.
    pub fn config(&self) -> &zenavif_tuner::EncoderConfig {
        self.tuned.config()
    }
}

/// Errors from the auto-tune consumer path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AvifAutotuneError {
    /// The supplied bake is not a parseable ZNPR v3 model, or does not
    /// declare a well-formed tune contract. Carries zenavif's message.
    Bake(String),
    /// No cell survived the masks, or feature resolution failed.
    Tune(String),
    /// The encoder rejected the tuned config or failed to encode.
    Encode(String),
}

impl core::fmt::Display for AvifAutotuneError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Bake(m) => write!(f, "avif auto-tune bake: {m}"),
            Self::Tune(m) => write!(f, "avif auto-tune: {m}"),
            Self::Encode(m) => write!(f, "avif auto-tune encode: {m}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AvifAutotuneError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    128,
                ]);
            }
        }
        px
    }

    #[test]
    fn stub_plan_encodes_and_explains_itself() {
        let (w, h) = (96, 96);
        let rgb = gradient(w, h);
        let tuner = AvifAutotune::stub();
        let plan = tuner
            .plan(&rgb, w, h, AvifIntent::new(80.0))
            .expect("a plan");
        let line = plan.explain();
        assert!(
            line.contains("source=stub"),
            "a stub decision must say so in the trace line, got: {line}"
        );
        assert!(line.contains("cell="));
        let avif = tuner.encode(&rgb, w, h, &plan).expect("encode");
        assert!(avif.len() > 32, "encode produced {} bytes", avif.len());
    }

    #[test]
    fn a_wrong_sized_buffer_is_refused_before_the_encoder_sees_it() {
        let tuner = AvifAutotune::stub();
        let rgb = gradient(32, 32);
        let plan = tuner.plan(&rgb, 32, 32, AvifIntent::new(80.0)).expect("plan");
        match tuner.encode(&rgb, 64, 64, &plan) {
            Err(AvifAutotuneError::Tune(_)) => {}
            Err(other) => panic!("wrong error variant: {other}"),
            Ok(_) => panic!("a buffer too short for the stated size must be refused"),
        }
    }

    #[test]
    fn an_unsatisfiable_budget_is_refused_not_silently_ignored() {
        // 1 ms for a 1 MP AVIF encode is not achievable by any measured
        // arm. The honest answer is a refusal.
        let (w, h) = (64, 64);
        let rgb = gradient(w, h);
        let intent = AvifIntent::new(80.0)
            .within_ms(0.0)
            .allowing(AllowedBackends::none().with(Av1Backend::Zenravif));
        match AvifAutotune::stub().plan(&rgb, 1000, 1000, intent) {
            Err(AvifAutotuneError::Tune(_)) => {}
            Err(other) => panic!("wrong error variant: {other}"),
            Ok(p) => panic!(
                "a 0 ms budget is unsatisfiable and must be refused, got: {}",
                p.explain()
            ),
        }
    }

    #[test]
    fn a_garbage_bake_fails_loudly_instead_of_falling_back_to_the_stub() {
        // `AvifAutotune` holds a parsed model and is deliberately not
        // `Debug`, so match rather than `expect_err`.
        match AvifAutotune::from_bake(b"not a znpr model at all") {
            Err(AvifAutotuneError::Bake(_)) => {}
            Err(other) => panic!("wrong error variant: {other}"),
            Ok(_) => panic!("garbage bytes must not load as a bake"),
        }
    }
}
