use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Solarize — invert pixels above a threshold.
///
/// Classic darkroom effect: pixels with luminance above the threshold
/// are inverted (1.0 - L), creating a partial negative. At threshold 0.0,
/// this is a full inversion; at 1.0, no pixels are affected.
///
/// Optionally affects chroma channels too, creating surreal color shifts.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Solarize {
    /// Threshold above which pixels are inverted. Range: 0.0–1.0.
    pub threshold: f32,
    /// Whether to also solarize chroma channels.
    pub solarize_chroma: bool,
}

impl Default for Solarize {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            solarize_chroma: false,
        }
    }
}

impl Filter for Solarize {
    fn channel_access(&self) -> ChannelAccess {
        if self.solarize_chroma {
            ChannelAccess::L_AND_CHROMA
        } else {
            ChannelAccess::L_ONLY
        }
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let t = self.threshold;

        for v in planes.l.iter_mut() {
            if *v > t {
                *v = 1.0 - *v;
            }
        }

        if self.solarize_chroma {
            // For chroma: invert sign when corresponding L was above threshold
            // We need to use L values, but L was already modified.
            // Instead, invert chroma values that are above the threshold magnitude.
            for v in planes.a.iter_mut() {
                if v.abs() > t * 0.5 {
                    *v = -*v;
                }
            }
            for v in planes.b.iter_mut() {
                if v.abs() > t * 0.5 {
                    *v = -*v;
                }
            }
        }
    }
}

static SOLARIZE_SCHEMA: FilterSchema = FilterSchema {
    name: "solarize",
    label: "Solarize",
    description: "Invert pixels above threshold (Sabattier effect)",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "threshold",
            label: "Threshold",
            description: "Luminance threshold for inversion (0 = full invert, 1 = no effect)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 1.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "solarize_chroma",
            label: "Solarize Chroma",
            description: "Also invert chroma channels above threshold",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for Solarize {
    fn schema() -> &'static FilterSchema {
        &SOLARIZE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "threshold" => Some(ParamValue::Float(self.threshold)),
            "solarize_chroma" => Some(ParamValue::Bool(self.solarize_chroma)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "threshold" => {
                if let Some(v) = value.as_f32() {
                    self.threshold = v.clamp(0.0, 1.0);
                    true
                } else {
                    false
                }
            }
            "solarize_chroma" => {
                if let ParamValue::Bool(v) = value {
                    self.solarize_chroma = v;
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
    fn threshold_one_is_identity() {
        let sol = Solarize {
            threshold: 1.0,
            solarize_chroma: false,
        };
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 256.0).clamp(0.0, 0.99);
        }
        let original = planes.l.clone();
        sol.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original, "threshold=1 should be identity");
    }

    #[test]
    fn threshold_zero_is_full_invert() {
        let sol = Solarize {
            threshold: 0.0,
            solarize_chroma: false,
        };
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.1;
        planes.l[1] = 0.5;
        planes.l[2] = 0.9;
        planes.l[3] = 0.0; // exactly at threshold — NOT inverted
        sol.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.l[0] - 0.9).abs() < 1e-6, "0.1 → 0.9");
        assert!((planes.l[1] - 0.5).abs() < 1e-6, "0.5 → 0.5");
        assert!((planes.l[2] - 0.1).abs() < 1e-6, "0.9 → 0.1");
        assert!((planes.l[3] - 0.0).abs() < 1e-6, "0.0 stays 0.0");
    }

    #[test]
    fn partial_solarize() {
        let sol = Solarize {
            threshold: 0.5,
            solarize_chroma: false,
        };
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.3; // below threshold → unchanged
        planes.l[1] = 0.7; // above threshold → inverted to 0.3
        sol.apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - 0.3).abs() < 1e-6,
            "below threshold unchanged"
        );
        assert!((planes.l[1] - 0.3).abs() < 1e-6, "above threshold inverted");
    }
}
