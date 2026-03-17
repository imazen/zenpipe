use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Split-toning / three-way color grading.
///
/// Applies different color tints to shadows, midtones, and highlights
/// independently. This is Lightroom's "Color Grading" panel.
///
/// Colors are specified as Oklab a/b offsets, which map naturally to
/// warm/cool (b axis) and green/magenta (a axis).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ColorGrading {
    /// Shadow tint: Oklab a offset.
    pub shadow_a: f32,
    /// Shadow tint: Oklab b offset.
    pub shadow_b: f32,
    /// Midtone tint: Oklab a offset.
    pub midtone_a: f32,
    /// Midtone tint: Oklab b offset.
    pub midtone_b: f32,
    /// Highlight tint: Oklab a offset.
    pub highlight_a: f32,
    /// Highlight tint: Oklab b offset.
    pub highlight_b: f32,
    /// Balance: shifts the shadow/highlight boundary.
    /// Negative = more shadow influence, positive = more highlight influence.
    /// Range: -1.0 to 1.0. Default: 0.0.
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            shadow_a: 0.0,
            shadow_b: 0.0,
            midtone_a: 0.0,
            midtone_b: 0.0,
            highlight_a: 0.0,
            highlight_b: 0.0,
            balance: 0.0,
        }
    }
}

static COLOR_GRADING_SCHEMA: FilterSchema = FilterSchema {
    name: "color_grading",
    label: "Color Grading",
    description: "Three-way split-toning for shadows, midtones, and highlights",
    group: FilterGroup::Color,
    params: &[
        ParamDesc {
            name: "shadow_a",
            label: "Shadow Green-Magenta",
            description: "Shadow tint on the a axis (green-magenta)",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Shadows",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "shadow_b",
            label: "Shadow Warm-Cool",
            description: "Shadow tint on the b axis (blue-yellow)",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Shadows",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "midtone_a",
            label: "Midtone Green-Magenta",
            description: "Midtone tint on the a axis",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Midtones",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "midtone_b",
            label: "Midtone Warm-Cool",
            description: "Midtone tint on the b axis",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Midtones",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "highlight_a",
            label: "Highlight Green-Magenta",
            description: "Highlight tint on the a axis",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Highlights",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "highlight_b",
            label: "Highlight Warm-Cool",
            description: "Highlight tint on the b axis",
            kind: ParamKind::Float {
                min: -0.1,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Highlights",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "balance",
            label: "Balance",
            description: "Shift shadow/highlight boundary (negative = more shadow influence)",
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
    ],
};

impl Describe for ColorGrading {
    fn schema() -> &'static FilterSchema {
        &COLOR_GRADING_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "shadow_a" => Some(ParamValue::Float(self.shadow_a)),
            "shadow_b" => Some(ParamValue::Float(self.shadow_b)),
            "midtone_a" => Some(ParamValue::Float(self.midtone_a)),
            "midtone_b" => Some(ParamValue::Float(self.midtone_b)),
            "highlight_a" => Some(ParamValue::Float(self.highlight_a)),
            "highlight_b" => Some(ParamValue::Float(self.highlight_b)),
            "balance" => Some(ParamValue::Float(self.balance)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "shadow_a" => self.shadow_a = v,
            "shadow_b" => self.shadow_b = v,
            "midtone_a" => self.midtone_a = v,
            "midtone_b" => self.midtone_b = v,
            "highlight_a" => self.highlight_a = v,
            "highlight_b" => self.highlight_b = v,
            "balance" => self.balance = v,
            _ => return false,
        }
        true
    }
}

impl ColorGrading {
    fn is_identity(&self) -> bool {
        self.shadow_a.abs() < 1e-6
            && self.shadow_b.abs() < 1e-6
            && self.midtone_a.abs() < 1e-6
            && self.midtone_b.abs() < 1e-6
            && self.highlight_a.abs() < 1e-6
            && self.highlight_b.abs() < 1e-6
    }
}

impl Filter for ColorGrading {
    fn channel_access(&self) -> ChannelAccess {
        // Reads L to determine tonal range, writes a/b
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let n = planes.pixel_count();
        // Balance shifts the crossover point between shadows and highlights.
        // Negative = expand shadow region (more shadow tint), positive = expand highlight region.
        let balance_offset = -self.balance * 0.25;
        let shadow_mid = (0.25 + balance_offset).clamp(0.1, 0.45);
        let mid_high = (0.75 + balance_offset).clamp(0.55, 0.9);

        for i in 0..n {
            let l = planes.l[i].clamp(0.0, 1.0);

            // Compute tonal weights using smooth transitions
            // Shadow weight: 1 at L=0, fades to 0 at shadow_mid
            let shadow_w = if l < shadow_mid {
                let t = l / shadow_mid;
                1.0 - t * t
            } else {
                0.0
            };

            // Highlight weight: 0 until mid_high, rises to 1 at L=1
            let highlight_w = if l > mid_high {
                let t = (l - mid_high) / (1.0 - mid_high);
                t * t
            } else {
                0.0
            };

            // Midtone weight: bell curve peaking between shadow_mid and mid_high
            let mid_center = (shadow_mid + mid_high) * 0.5;
            let mid_width = (mid_high - shadow_mid) * 0.5;
            let mid_t = ((l - mid_center) / mid_width).abs().min(1.0);
            let midtone_w = 1.0 - mid_t * mid_t;

            planes.a[i] += shadow_w * self.shadow_a
                + midtone_w * self.midtone_a
                + highlight_w * self.highlight_a;
            planes.b[i] += shadow_w * self.shadow_b
                + midtone_w * self.midtone_b
                + highlight_w * self.highlight_b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        ColorGrading::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn shadow_tint_affects_dark_pixels() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.05; // dark
        planes.l[1] = 0.9; // bright
        planes.a[0] = 0.0;
        planes.a[1] = 0.0;

        let mut cg = ColorGrading::default();
        cg.shadow_a = 0.05; // tint shadows toward magenta
        cg.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.a[0] > 0.03,
            "dark pixel should be tinted: {}",
            planes.a[0]
        );
        assert!(
            planes.a[1].abs() < 0.01,
            "bright pixel should be barely affected: {}",
            planes.a[1]
        );
    }

    #[test]
    fn highlight_tint_affects_bright_pixels() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.05;
        planes.l[1] = 0.95;
        planes.b[0] = 0.0;
        planes.b[1] = 0.0;

        let mut cg = ColorGrading::default();
        cg.highlight_b = 0.05; // warm highlights
        cg.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.b[1] > 0.03,
            "bright pixel should be tinted: {}",
            planes.b[1]
        );
        assert!(
            planes.b[0].abs() < 0.01,
            "dark pixel should be barely affected: {}",
            planes.b[0]
        );
    }

    #[test]
    fn does_not_modify_luminance() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let l_orig = planes.l.clone();
        let mut cg = ColorGrading::default();
        cg.shadow_a = 0.1;
        cg.highlight_b = 0.1;
        cg.midtone_a = -0.05;
        cg.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }

    #[test]
    fn balance_shifts_crossover() {
        // With positive balance, shadows region shrinks — a mid-dark pixel
        // gets less shadow tint. Negative balance expands it.
        let mut planes_pos = OklabPlanes::new(1, 1);
        planes_pos.l[0] = 0.15; // clearly in shadow range
        planes_pos.a[0] = 0.0;

        let mut planes_neg = planes_pos.clone();

        let mut cg_pos = ColorGrading::default();
        cg_pos.shadow_a = 0.1;
        cg_pos.balance = 0.5;
        cg_pos.apply(&mut planes_pos, &mut FilterContext::new());

        let mut cg_neg = ColorGrading::default();
        cg_neg.shadow_a = 0.1;
        cg_neg.balance = -0.5;
        cg_neg.apply(&mut planes_neg, &mut FilterContext::new());

        // Both should be tinted, but negative balance should give MORE
        assert!(
            planes_neg.a[0] > planes_pos.a[0],
            "negative balance should increase shadow influence: neg={} pos={}",
            planes_neg.a[0],
            planes_pos.a[0]
        );
    }
}
