//! Perceptual-metric-gated filtering: apply → score → scale back / skip.
//!
//! [`MetricGated`] wraps any [`Filter`] with a quality gate. It runs the inner
//! filter, measures the perceptual distance between the original and filtered
//! image with a pluggable [`QualityMetric`], and — if the change exceeds
//! `max_distance` — blends the edit back toward the original by the largest
//! factor that keeps the change just under the threshold (a binary search). If
//! even a tiny edit is over threshold, the edit is skipped entirely.
//!
//! This is the "checks and balances" layer for automated pipelines: a filter
//! can be aggressive by design, and the gate guarantees the *visible* change
//! stays below a just-noticeable bound.
//!
//! The metric is pluggable so heavyweight perceptual metrics (e.g. zensim) can
//! be supplied as a closure without pulling them into this crate's
//! dependencies — any `Fn(&OklabPlanes, &OklabPlanes) -> f32` is a
//! [`QualityMetric`]:
//!
//! ```ignore
//! use zenfilters::metric_gate::MetricGated;
//! let gated = MetricGated {
//!     filter: Box::new(my_filter),
//!     // higher zensim score = more similar, so distance = 100 - score
//!     metric: |orig: &OklabPlanes, cand: &OklabPlanes| 100.0 - zensim_score(orig, cand),
//!     max_distance: 8.0,
//!     iterations: 7,
//! };
//! ```
//!
//! A zero-dependency [`OklabDeltaMetric`] (mean Oklab ΔE) is provided as a
//! default for standalone use and tests.

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// A perceptual distance between an original image and a candidate edit.
///
/// `0.0` means identical; larger means a more visible change. Implemented for
/// any `Fn(&OklabPlanes, &OklabPlanes) -> f32`, so closures (including ones
/// wrapping an external metric like zensim) work directly.
pub trait QualityMetric {
    /// Distance between `original` and `candidate` (same dimensions).
    fn distance(&self, original: &OklabPlanes, candidate: &OklabPlanes) -> f32;
}

impl<F> QualityMetric for F
where
    F: Fn(&OklabPlanes, &OklabPlanes) -> f32,
{
    #[inline]
    fn distance(&self, original: &OklabPlanes, candidate: &OklabPlanes) -> f32 {
        self(original, candidate)
    }
}

/// Mean Oklab ΔE (Euclidean distance in L/a/b) over all pixels.
///
/// Oklab is approximately perceptually uniform, so this is a cheap,
/// zero-dependency stand-in for a full perceptual metric. It scales linearly
/// with edit strength, which makes the gate's binary search exact.
#[derive(Clone, Copy, Debug, Default)]
pub struct OklabDeltaMetric;

impl QualityMetric for OklabDeltaMetric {
    fn distance(&self, original: &OklabPlanes, candidate: &OklabPlanes) -> f32 {
        let n = original.l.len().min(candidate.l.len());
        if n == 0 {
            return 0.0;
        }
        let mut sum = 0.0f64;
        for i in 0..n {
            let dl = candidate.l[i] - original.l[i];
            let da = candidate.a[i] - original.a[i];
            let db = candidate.b[i] - original.b[i];
            sum += (dl * dl + da * da + db * db).sqrt() as f64;
        }
        (sum / n as f64) as f32
    }
}

/// Wraps a [`Filter`] with a perceptual quality gate (see module docs).
pub struct MetricGated<M: QualityMetric> {
    /// The filter to apply, then gate.
    pub filter: Box<dyn Filter>,
    /// The perceptual distance metric.
    pub metric: M,
    /// Maximum allowed perceptual distance. The edit is scaled back to keep the
    /// measured distance at or below this; `0.0` always skips.
    pub max_distance: f32,
    /// Binary-search refinement steps used to find the scale-back factor.
    /// `6`–`8` is plenty. Each step evaluates the metric once.
    pub iterations: u32,
}

/// `dst = lerp(orig, filtered, s)` per plane.
fn blend_into(dst: &mut OklabPlanes, orig: &OklabPlanes, filtered: &OklabPlanes, s: f32) {
    let inv = 1.0 - s;
    for i in 0..dst.l.len() {
        dst.l[i] = orig.l[i] * inv + filtered.l[i] * s;
    }
    for i in 0..dst.a.len() {
        dst.a[i] = orig.a[i] * inv + filtered.a[i] * s;
    }
    for i in 0..dst.b.len() {
        dst.b[i] = orig.b[i] * inv + filtered.b[i] * s;
    }
    if let (Some(da), Some(oa), Some(fa)) = (
        dst.alpha.as_mut(),
        orig.alpha.as_ref(),
        filtered.alpha.as_ref(),
    ) {
        for i in 0..da.len() {
            da[i] = oa[i] * inv + fa[i] * s;
        }
    }
}

impl<M: QualityMetric + Send + Sync> Filter for MetricGated<M> {
    fn channel_access(&self) -> ChannelAccess {
        self.filter.channel_access()
    }

    fn is_neighborhood(&self) -> bool {
        self.filter.is_neighborhood()
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        self.filter.neighborhood_radius(width, height)
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        self.filter.tag()
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        self.filter.resize_phase()
    }

    fn scale_for_resolution(&mut self, scale: f32) {
        self.filter.scale_for_resolution(scale);
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        self.filter.plane_semantics()
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        // Snapshot the original, run the inner filter, snapshot the result.
        let orig = planes.clone();
        self.filter.apply(planes, ctx);

        // Full-strength change acceptable? Keep it.
        let full = self.metric.distance(&orig, planes);
        if full <= self.max_distance {
            return;
        }
        // Threshold of zero (or negative): skip entirely.
        if self.max_distance <= 0.0 {
            *planes = orig;
            return;
        }

        let filtered = planes.clone();

        // Binary-search the largest blend factor `s` whose distance is within
        // budget. `lo` is always known-acceptable (s=0 ⇒ original ⇒ distance 0).
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        for _ in 0..self.iterations.max(1) {
            let mid = 0.5 * (lo + hi);
            blend_into(planes, &orig, &filtered, mid);
            let d = self.metric.distance(&orig, planes);
            if d <= self.max_distance {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        // Apply the largest known-acceptable blend.
        blend_into(planes, &orig, &filtered, lo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::Exposure;

    fn mid_gray(w: u32, h: u32) -> OklabPlanes {
        let mut p = OklabPlanes::new(w, h);
        for i in 0..(w * h) as usize {
            p.l[i] = 0.5;
            p.a[i] = 0.02;
            p.b[i] = -0.01;
        }
        p
    }

    #[test]
    fn passes_small_change_unchanged() {
        // A generous budget keeps the full effect.
        let mut p = mid_gray(32, 32);
        let mut reference = p.clone();
        Exposure { stops: 0.3 }.apply(&mut reference, &mut FilterContext::new());

        let gated = MetricGated {
            filter: Box::new(Exposure { stops: 0.3 }),
            metric: OklabDeltaMetric,
            max_distance: 10.0, // huge — never triggers
            iterations: 8,
        };
        gated.apply(&mut p, &mut FilterContext::new());

        let mut max_err = 0.0f32;
        for i in 0..p.l.len() {
            max_err = max_err.max((p.l[i] - reference.l[i]).abs());
        }
        assert!(
            max_err < 1e-6,
            "full effect should be kept, max_err={max_err}"
        );
    }

    #[test]
    fn scales_back_large_change() {
        let orig = mid_gray(32, 32);
        let mut p = orig.clone();
        let budget = 0.03f32;
        let gated = MetricGated {
            filter: Box::new(Exposure { stops: 1.5 }),
            metric: OklabDeltaMetric,
            max_distance: budget,
            iterations: 10,
        };
        gated.apply(&mut p, &mut FilterContext::new());

        let d = OklabDeltaMetric.distance(&orig, &p);
        // Scaled back to (just under) the budget.
        assert!(
            d <= budget + 1e-3,
            "scaled-back distance {d} should be within budget {budget}"
        );
        assert!(d > budget * 0.5, "should keep as much effect as fits: {d}");
        // And it's not a full no-op.
        let mut changed = false;
        for i in 0..p.l.len() {
            if (p.l[i] - orig.l[i]).abs() > 1e-4 {
                changed = true;
                break;
            }
        }
        assert!(changed, "some effect should remain");
    }

    #[test]
    fn zero_budget_skips() {
        let orig = mid_gray(16, 16);
        let mut p = orig.clone();
        let gated = MetricGated {
            filter: Box::new(Exposure { stops: 2.0 }),
            metric: OklabDeltaMetric,
            max_distance: 0.0,
            iterations: 8,
        };
        gated.apply(&mut p, &mut FilterContext::new());
        assert_eq!(p.l, orig.l, "zero budget must skip the edit entirely");
    }

    #[test]
    fn closure_metric_works() {
        // A closure metric (here, the same mean-ΔE) plugs in directly.
        let orig = mid_gray(16, 16);
        let mut p = orig.clone();
        let metric = |o: &OklabPlanes, c: &OklabPlanes| {
            let mut s = 0.0f32;
            for i in 0..o.l.len() {
                s += (c.l[i] - o.l[i]).abs();
            }
            s / o.l.len() as f32
        };
        let gated = MetricGated {
            filter: Box::new(Exposure { stops: 1.0 }),
            metric,
            max_distance: 0.02,
            iterations: 10,
        };
        gated.apply(&mut p, &mut FilterContext::new());
        let d = {
            let mut s = 0.0f32;
            for i in 0..orig.l.len() {
                s += (p.l[i] - orig.l[i]).abs();
            }
            s / orig.l.len() as f32
        };
        assert!(d <= 0.02 + 1e-3, "closure-gated distance {d} within budget");
    }
}
