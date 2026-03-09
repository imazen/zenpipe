use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Gamut expansion: hue-selective chroma boost simulating wider color gamuts.
///
/// Display P3 extends significantly beyond sRGB in specific hue regions:
/// - Red-orange: the biggest difference (vivid reds, rich oranges)
/// - Green: moderately wider (deeper greens)
/// - Blue: slightly wider
///
/// This filter selectively boosts chroma in the hue regions where P3
/// extends beyond sRGB, producing the "P3 look" — more vivid reds,
/// richer greens, and punchier oranges — that modern phones display.
///
/// Unlike uniform saturation, this targets only the hues that benefit
/// from expansion, preserving the balance of other colors.
///
/// The boost amount adapts per-pixel: already-saturated colors get less
/// boost (vibrance-style protection) to prevent clipping.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GamutExpand {
    /// Expansion strength. 0.0 = sRGB, 1.0 = full P3-like expansion.
    pub strength: f32,
}

impl Default for GamutExpand {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

/// Hue-selective expansion weights for P3 vs sRGB gamut difference.
///
/// These approximate how much wider P3 is than sRGB at each hue angle
/// in Oklab space. Values are relative: 1.0 = maximum expansion region.
///
/// Computed from the P3-sRGB gamut boundary difference at L=0.6.
/// The actual boundary varies with L, but this captures the shape.
fn p3_expansion_weight(a: f32, b: f32) -> f32 {
    // Hue angle in Oklab a/b space
    let hue = b.atan2(a); // -π to π

    // P3 extends beyond sRGB most in these regions:
    // - Red-orange: hue ≈ 0.5 to 1.2 rad (peak at ~0.8)  → biggest difference
    // - Green:      hue ≈ 2.2 to 2.9 rad (peak at ~2.5)  → moderate difference
    // - Blue:       hue ≈ -1.8 to -1.2 rad               → small difference
    //
    // Use smooth bell curves centered on each peak.

    let red_orange = gaussian_bell(hue, 0.8, 0.5); // center 0.8, width 0.5
    let green = gaussian_bell(hue, 2.5, 0.4) * 0.6; // slightly less expansion
    let blue = gaussian_bell(hue, -1.5, 0.4) * 0.3; // minimal expansion

    (red_orange + green + blue).min(1.0)
}

/// Smooth bell curve: exp(-0.5 * ((x - center) / width)²)
#[inline]
fn gaussian_bell(x: f32, center: f32, width: f32) -> f32 {
    let d = (x - center) / width;
    (-0.5 * d * d).exp()
}

impl Filter for GamutExpand {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let s = self.strength;
        // Maximum chroma expansion factor (at strength=1 in peak P3 region)
        // P3 is roughly 25% wider than sRGB in the red-orange axis.
        let max_expansion = 0.25;

        // Vibrance-style protection: already-saturated colors get less boost
        const MAX_CHROMA: f32 = 0.4;

        for (a_val, b_val) in planes.a.iter_mut().zip(planes.b.iter_mut()) {
            let a = *a_val;
            let b = *b_val;

            let chroma = (a * a + b * b).sqrt();
            if chroma < 1e-7 {
                continue; // near-neutral, skip
            }

            // Hue-selective weight: how much P3 extends beyond sRGB here
            let hue_weight = p3_expansion_weight(a, b);
            if hue_weight < 0.01 {
                continue; // hue not in P3 extension region
            }

            // Protection: reduce boost for already-saturated colors
            let protection = 1.0 - (chroma / MAX_CHROMA).min(1.0);

            // Final expansion factor
            let expansion = 1.0 + s * max_expansion * hue_weight * protection;

            *a_val = a * expansion;
            *b_val = b * expansion;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = 0.05;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        GamutExpand { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn boosts_red_orange() {
        // Red-orange in Oklab: positive a, positive b
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.15; // red-ish
        planes.b[0] = 0.12; // orange-ish
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        assert!(
            chroma_after > chroma_before,
            "red-orange should be boosted: {chroma_before} -> {chroma_after}"
        );
    }

    #[test]
    fn boosts_green() {
        // Green in Oklab: negative a, positive b
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = -0.12;
        planes.b[0] = 0.10;
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        assert!(
            chroma_after > chroma_before,
            "green should be boosted: {chroma_before} -> {chroma_after}"
        );
    }

    #[test]
    fn minimal_effect_on_yellow() {
        // Yellow in Oklab: near zero a, positive b (between red and green peaks)
        // This is NOT a major P3 extension region
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.01;
        planes.b[0] = 0.15;
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        // Yellow should get very little boost (P3 barely extends in yellow)
        let boost_pct = (chroma_after - chroma_before) / chroma_before * 100.0;
        assert!(
            boost_pct < 10.0,
            "yellow should get minimal boost: {boost_pct:.1}%"
        );
    }

    #[test]
    fn protects_already_saturated() {
        // Highly saturated red — should get less boost
        let mut planes_low = OklabPlanes::new(1, 1);
        planes_low.a[0] = 0.05;
        planes_low.b[0] = 0.04;
        let chroma_low_before = (planes_low.a[0].powi(2) + planes_low.b[0].powi(2)).sqrt();

        let mut planes_high = OklabPlanes::new(1, 1);
        planes_high.a[0] = 0.30;
        planes_high.b[0] = 0.24;
        let chroma_high_before = (planes_high.a[0].powi(2) + planes_high.b[0].powi(2)).sqrt();

        GamutExpand { strength: 1.0 }.apply(&mut planes_low, &mut FilterContext::new());
        GamutExpand { strength: 1.0 }.apply(&mut planes_high, &mut FilterContext::new());

        let chroma_low_after = (planes_low.a[0].powi(2) + planes_low.b[0].powi(2)).sqrt();
        let chroma_high_after = (planes_high.a[0].powi(2) + planes_high.b[0].powi(2)).sqrt();

        let pct_low = (chroma_low_after - chroma_low_before) / chroma_low_before;
        let pct_high = (chroma_high_after - chroma_high_before) / chroma_high_before;

        assert!(
            pct_low > pct_high,
            "low chroma should get more boost: {pct_low:.3} vs {pct_high:.3}"
        );
    }

    #[test]
    fn does_not_modify_luminance() {
        let mut planes = OklabPlanes::new(10, 1);
        for v in &mut planes.l {
            *v = 0.6;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let l_orig = planes.l.clone();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }

    #[test]
    fn preserves_hue() {
        // Expansion should scale chroma without changing hue angle
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.12;
        planes.b[0] = 0.10;
        let hue_before = planes.b[0].atan2(planes.a[0]);
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let hue_after = planes.b[0].atan2(planes.a[0]);
        assert!(
            (hue_after - hue_before).abs() < 1e-5,
            "hue should be preserved: {hue_before} -> {hue_after}"
        );
    }

    #[test]
    fn neutral_unchanged() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a[0], 0.0);
        assert_eq!(planes.b[0], 0.0);
    }
}
