use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Post-crop vignette: darken or lighten image edges.
///
/// Applies a radial falloff from center to edges. The falloff shape is
/// controlled by `feather` (softness) and `roundness` (circle vs rectangle).
///
/// Positive strength darkens edges (classic vignette), negative brightens.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Vignette {
    /// Vignette strength. Positive darkens edges, negative brightens.
    /// Typical: 0.3 to 0.8.
    pub strength: f32,
    /// Midpoint: how far from center the effect starts. 0.0 = center, 1.0 = corners.
    /// Default: 0.5.
    pub midpoint: f32,
    /// Feather: softness of the transition. 0.0 = hard edge, 1.0 = very soft.
    /// Default: 0.5.
    pub feather: f32,
    /// Roundness: 1.0 = circular, 0.0 = rectangular. Default: 1.0.
    pub roundness: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            strength: 0.0,
            midpoint: 0.5,
            feather: 0.5,
            roundness: 1.0,
        }
    }
}

impl Filter for Vignette {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;
        let cx = w as f32 * 0.5;
        let cy = h as f32 * 0.5;
        // Normalize so corner distance = 1.0
        let diag = (cx * cx + cy * cy).sqrt();
        if diag < 1.0 {
            return;
        }
        let inv_diag = 1.0 / diag;

        let midpoint = self.midpoint.clamp(0.01, 0.99);
        let feather = self.feather.clamp(0.01, 1.0);
        // Map feather to a transition width around the midpoint
        let transition = feather * 0.5;
        let edge_start = midpoint - transition;
        let edge_end = midpoint + transition;
        let inv_transition = if edge_end > edge_start {
            1.0 / (edge_end - edge_start)
        } else {
            1.0
        };
        let roundness = self.roundness.clamp(0.0, 1.0);

        for y in 0..h {
            let dy = (y as f32 + 0.5 - cy) * inv_diag;
            for x in 0..w {
                let dx = (x as f32 + 0.5 - cx) * inv_diag;

                // Distance: blend between rectangular (max) and circular (euclidean)
                let d_circ = (dx * dx + dy * dy).sqrt();
                let d_rect = dx.abs().max(dy.abs()) * core::f32::consts::SQRT_2;
                let d = roundness * d_circ + (1.0 - roundness) * d_rect;

                // Compute falloff: 0 inside midpoint, 1 at/beyond edge
                let t = ((d - edge_start) * inv_transition).clamp(0.0, 1.0);
                // Smooth hermite interpolation
                let falloff = t * t * (3.0 - 2.0 * t);

                // Apply: darken = multiply L by (1 - strength * falloff)
                let factor = 1.0 - self.strength * falloff;
                let idx = y * w + x;
                planes.l[idx] *= factor;
                // Scale chroma proportionally to maintain color appearance
                planes.a[idx] *= factor;
                planes.b[idx] *= factor;
            }
        }
    }
}

static VIGNETTE_SCHEMA: FilterSchema = FilterSchema {
    name: "vignette",
    label: "Vignette",
    description: "Post-crop vignette with adjustable shape and falloff",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Vignette strength (positive = darken edges, negative = brighten)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "midpoint",
            label: "Midpoint",
            description: "Distance from center where effect starts (0 = center, 1 = corners)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "feather",
            label: "Feather",
            description: "Transition softness (0 = hard, 1 = very soft)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "roundness",
            label: "Roundness",
            description: "Shape (1 = circular, 0 = rectangular)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Vignette {
    fn schema() -> &'static FilterSchema {
        &VIGNETTE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            "midpoint" => Some(ParamValue::Float(self.midpoint)),
            "feather" => Some(ParamValue::Float(self.feather)),
            "roundness" => Some(ParamValue::Float(self.roundness)),
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
            "midpoint" => self.midpoint = v,
            "feather" => self.feather = v,
            "roundness" => self.roundness = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let orig = planes.l.clone();
        Vignette::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn positive_darkens_edges() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let mut vig = Vignette::default();
        vig.strength = 0.5;
        vig.apply(&mut planes, &mut FilterContext::new());

        let center = planes.l[32 * 64 + 32];
        let corner = planes.l[0]; // top-left corner
        assert!(
            corner < center,
            "corner ({corner}) should be darker than center ({center})"
        );
    }

    #[test]
    fn negative_brightens_edges() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let mut vig = Vignette::default();
        vig.strength = -0.5;
        vig.apply(&mut planes, &mut FilterContext::new());

        let center = planes.l[32 * 64 + 32];
        let corner = planes.l[0];
        assert!(
            corner > center,
            "corner ({corner}) should be brighter than center ({center})"
        );
    }

    #[test]
    fn center_pixel_least_affected() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.6;
        }
        let mut vig = Vignette::default();
        vig.strength = 0.8;
        vig.apply(&mut planes, &mut FilterContext::new());

        let center = planes.l[32 * 64 + 32];
        // Center should be close to original
        assert!(
            (center - 0.6).abs() < 0.05,
            "center should be near original: {center}"
        );
    }

    #[test]
    fn scales_chroma_with_luminance() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let mut vig = Vignette::default();
        vig.strength = 0.5;
        vig.apply(&mut planes, &mut FilterContext::new());

        // Corner chroma should be reduced proportionally
        let corner_a = planes.a[0];
        let center_a = planes.a[32 * 64 + 32];
        assert!(
            corner_a < center_a,
            "corner chroma ({corner_a}) should be less than center ({center_a})"
        );
    }
}
