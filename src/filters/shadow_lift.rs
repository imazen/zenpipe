use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Automatic shadow lift: recovers detail in crushed dark areas.
///
/// Analyzes the L histogram to detect how much shadow content is crushed,
/// then applies a proportional toe lift curve. Images with properly
/// exposed shadows are barely affected.
///
/// This fixes the second most common phone camera failure: dark faces
/// in backlit scenes, lost detail in shadows and corners, and the
/// "everything below the subject is black" problem.
///
/// Algorithm:
/// 1. Compute histogram percentiles (p1, p5)
/// 2. If significant shadow content exists, compute toe point
/// 3. Apply toe lift: below toe, L' = toe * (L/toe)^(1/(1+lift))
///    where lift is proportional to strength and shadow density
///
/// The result is a smooth toe curve that opens up shadows without
/// affecting midtones or highlights. More crushed shadows = more lift.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ShadowLift {
    /// Lift strength. 0.0 = off, 1.0 = full lift.
    pub strength: f32,
}

impl Default for ShadowLift {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

impl Filter for ShadowLift {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::ShadowLift
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        // Histogram analysis: find p1 and p5
        let (_p1, p5) = percentiles(&planes.l, &[0.01, 0.05]);

        // If shadows are already open (p5 > 0.15), minimal work needed
        if p5 > 0.3 {
            return;
        }

        // Adaptive toe: higher toe when more shadow content is crushed.
        // If p5 ≈ p1 (shadows are hard-clipped), use a higher toe for more recovery.
        // If p5 >> p1 (gradual shadow rolloff), use a lower toe.
        let shadow_density = (0.3 - p5).max(0.0) / 0.3; // 0-1: how crushed are shadows
        let toe = (p5 + (0.3 - p5) * self.strength * shadow_density).clamp(0.02, 0.4);

        // Toe lift: gamma curve below toe point
        // L' = toe * (L/toe)^gamma where gamma = 1/(1 + lift)
        // lift = 0 → gamma = 1 (identity), lift > 0 → gamma < 1 (brighten)
        let lift = self.strength * shadow_density * 0.8; // tuned for natural results
        if lift < 1e-6 {
            return;
        }
        let gamma = 1.0 / (1.0 + lift);
        let inv_toe = 1.0 / toe;

        for v in planes.l.iter_mut() {
            if *v < toe && *v > 0.0 {
                // Normalized position in shadow region
                let t = *v * inv_toe;
                *v = toe * t.powf(gamma);
            }
        }
    }
}

static SHADOW_LIFT_SCHEMA: FilterSchema = FilterSchema {
    name: "shadow_lift",
    label: "Shadow Lift",
    description: "Automatic toe-curve recovery for crushed shadows",
    group: FilterGroup::ToneRange,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Lift strength (0 = off, 1 = full)",
        kind: ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            identity: 0.0,
            step: 0.05,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for ShadowLift {
    fn schema() -> &'static FilterSchema {
        &SHADOW_LIFT_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "strength" => self.strength = v,
            _ => return false,
        }
        true
    }
}

/// Compute percentiles from a slice using approximate histogram.
fn percentiles(data: &[f32], pcts: &[f64]) -> (f32, f32) {
    const BINS: usize = 1024;
    let mut hist = [0u32; BINS];
    for &v in data {
        let bin = ((v.max(0.0) * BINS as f32) as usize).min(BINS - 1);
        hist[bin] += 1;
    }

    let n = data.len() as f64;
    let mut results = [0.0f32; 2];
    for (ri, &pct) in pcts.iter().enumerate() {
        let target = (n * pct) as u64;
        let mut cumsum = 0u64;
        for (bin, &count) in hist.iter().enumerate() {
            cumsum += count as u64;
            if cumsum >= target {
                results[ri] = bin as f32 / BINS as f32;
                break;
            }
        }
    }
    (results[0], results[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let original = planes.l.clone();
        ShadowLift { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn lifts_crushed_shadows() {
        let mut planes = OklabPlanes::new(100, 1);
        // 30% of pixels are crushed shadows, 70% are midtones
        for i in 0..30 {
            planes.l[i] = 0.01 + (i as f32 / 30.0) * 0.04; // 0.01-0.05
        }
        for i in 30..100 {
            planes.l[i] = 0.3 + (i as f32 - 30.0) / 70.0 * 0.5; // 0.3-0.8
        }
        let dark_sum_before: f32 = planes.l[0..30].iter().sum();
        ShadowLift { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let dark_sum_after: f32 = planes.l[0..30].iter().sum();
        // Shadows should be lifted (sum increases)
        assert!(
            dark_sum_after > dark_sum_before,
            "shadows should be lifted: {dark_sum_before} -> {dark_sum_after}"
        );
    }

    #[test]
    fn does_not_affect_bright_images() {
        let mut planes = OklabPlanes::new(100, 1);
        // All pixels well-exposed — no shadows to lift
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.4 + (i as f32 / 100.0) * 0.5; // 0.4-0.9
        }
        let original = planes.l.clone();
        ShadowLift { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original, "bright images should not be affected");
    }

    #[test]
    fn preserves_midtones() {
        let mut planes = OklabPlanes::new(100, 1);
        for i in 0..30 {
            planes.l[i] = 0.02;
        }
        for i in 30..100 {
            planes.l[i] = 0.5;
        }
        let mid_before = planes.l[50];
        ShadowLift { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let mid_after = planes.l[50];
        assert!(
            (mid_after - mid_before).abs() < 0.01,
            "midtones should be preserved: {mid_before} -> {mid_after}"
        );
    }

    #[test]
    fn maintains_monotonicity() {
        let mut planes = OklabPlanes::new(100, 1);
        for i in 0..100 {
            planes.l[i] = (i as f32 / 100.0) * 0.3; // 0.0-0.3, all in shadow
        }
        ShadowLift { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        for i in 1..100 {
            assert!(
                planes.l[i] >= planes.l[i - 1] - 1e-6,
                "should remain monotonic at {i}: {} < {}",
                planes.l[i],
                planes.l[i - 1]
            );
        }
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.05;
        }
        for v in &mut planes.a {
            *v = 0.03;
        }
        let a_orig = planes.a.clone();
        ShadowLift { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn stronger_lifts_more() {
        let make = || {
            let mut planes = OklabPlanes::new(100, 1);
            for i in 0..40 {
                planes.l[i] = 0.02;
            }
            for i in 40..100 {
                planes.l[i] = 0.5;
            }
            planes
        };

        let mut planes_weak = make();
        let mut planes_strong = make();
        ShadowLift { strength: 0.3 }.apply(&mut planes_weak, &mut FilterContext::new());
        ShadowLift { strength: 1.0 }.apply(&mut planes_strong, &mut FilterContext::new());

        let dark_weak = planes_weak.l[10];
        let dark_strong = planes_strong.l[10];
        assert!(
            dark_strong > dark_weak,
            "stronger lift should brighten more: weak={dark_weak} strong={dark_strong}"
        );
    }
}
