use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Posterize — reduce the number of distinct luminance/color levels.
///
/// Quantizes pixel values to a fixed number of steps, creating
/// flat regions of uniform tone. Common artistic effect that reduces
/// gradients to distinct bands.
///
/// Operates in Oklab: quantizing L produces clean luminance steps,
/// quantizing chroma produces discrete color regions.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Posterize {
    /// Number of luminance levels (2–256). 2 = binary, 4–8 = strong effect.
    pub levels: u32,
    /// Whether to also posterize chroma channels.
    pub posterize_chroma: bool,
}

impl Default for Posterize {
    fn default() -> Self {
        Self {
            levels: 4,
            posterize_chroma: false,
        }
    }
}

impl Filter for Posterize {
    fn channel_access(&self) -> ChannelAccess {
        if self.posterize_chroma {
            ChannelAccess::L_AND_CHROMA
        } else {
            ChannelAccess::L_ONLY
        }
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let levels = self.levels.max(2) as f32;
        let steps = levels - 1.0;

        // Quantize L
        for v in planes.l.iter_mut() {
            *v = (*v * steps).round() / steps;
        }

        if self.posterize_chroma {
            // Chroma range is roughly [-0.5, 0.5]; map to [0, 1] for quantization
            for v in planes.a.iter_mut() {
                let norm = (*v + 0.5).clamp(0.0, 1.0);
                *v = (norm * steps).round() / steps - 0.5;
            }
            for v in planes.b.iter_mut() {
                let norm = (*v + 0.5).clamp(0.0, 1.0);
                *v = (norm * steps).round() / steps - 0.5;
            }
        }
    }
}

static POSTERIZE_SCHEMA: FilterSchema = FilterSchema {
    name: "posterize",
    label: "Posterize",
    description: "Reduce to N distinct luminance/color levels",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "levels",
            label: "Levels",
            description: "Number of output levels (2 = binary, 4–8 typical)",
            kind: ParamKind::Int {
                min: 2,
                max: 256,
                default: 4,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "posterize_chroma",
            label: "Posterize Chroma",
            description: "Also quantize color channels",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for Posterize {
    fn schema() -> &'static FilterSchema {
        &POSTERIZE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "levels" => Some(ParamValue::Int(self.levels as i32)),
            "posterize_chroma" => Some(ParamValue::Bool(self.posterize_chroma)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "levels" => {
                if let Some(v) = value.as_i32() {
                    self.levels = (v as u32).clamp(2, 256);
                    true
                } else {
                    false
                }
            }
            "posterize_chroma" => {
                if let ParamValue::Bool(v) = value {
                    self.posterize_chroma = v;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn binary_posterize() {
        let post = Posterize {
            levels: 2,
            posterize_chroma: false,
        };
        let mut planes = OklabPlanes::new(16, 16);
        // Gradient
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        post.apply(&mut planes, &mut FilterContext::new());
        // All values should be 0.0 or 1.0
        for &v in &planes.l {
            assert!(
                v == 0.0 || v == 1.0,
                "binary posterize should produce 0 or 1, got {v}"
            );
        }
    }

    #[test]
    fn four_levels() {
        let post = Posterize {
            levels: 4,
            posterize_chroma: false,
        };
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.1; // → round(0.1*3)/3 = 0/3 = 0.0
        planes.l[1] = 0.3; // → round(0.3*3)/3 = 1/3 ≈ 0.333
        planes.l[2] = 0.6; // → round(0.6*3)/3 = 2/3 ≈ 0.667
        planes.l[3] = 0.9; // → round(0.9*3)/3 = 3/3 = 1.0
        post.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.l[0] - 0.0).abs() < 0.01);
        assert!((planes.l[1] - 1.0 / 3.0).abs() < 0.01);
        assert!((planes.l[2] - 2.0 / 3.0).abs() < 0.01);
        assert!((planes.l[3] - 1.0).abs() < 0.01);
    }

    #[test]
    fn chroma_posterize() {
        let post = Posterize {
            levels: 2,
            posterize_chroma: true,
        };
        let mut planes = OklabPlanes::new(4, 4);
        planes.a[0] = 0.1; // → (0.6 mapped to [0,1]) → round → 1.0 → back to 0.5
        planes.a[1] = -0.1; // → (0.4 mapped) → round → 0.0 → back to -0.5
        post.apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.a[0] == 0.5 || planes.a[0] == -0.5,
            "chroma should be binary, got {}",
            planes.a[0]
        );
    }

    #[test]
    fn high_levels_near_identity() {
        let post = Posterize {
            levels: 256,
            posterize_chroma: false,
        };
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        post.apply(&mut planes, &mut FilterContext::new());
        for (i, (&got, &expected)) in planes.l.iter().zip(original.iter()).enumerate() {
            assert!(
                (got - expected).abs() < 0.005,
                "256 levels should be near-identity: pixel {i}, expected {expected}, got {got}"
            );
        }
    }
}
