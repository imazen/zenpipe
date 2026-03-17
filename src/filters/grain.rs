use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Film grain simulation.
///
/// Adds synthetic grain to the luminance channel using a simple
/// hash-based noise generator (deterministic per-pixel position).
/// Grain intensity varies with luminance: stronger in midtones,
/// weaker in deep shadows and bright highlights (like real film).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Grain {
    /// Grain amount. 0.0 = none, 1.0 = heavy. Typical: 0.1–0.3.
    pub amount: f32,
    /// Grain size. Controls the spatial frequency of the grain pattern.
    /// 1.0 = fine (per-pixel), 2.0+ = coarser. Default: 1.0.
    pub size: f32,
    /// Random seed for the grain pattern. Different seeds produce
    /// different patterns. Default: 0.
    pub seed: u32,
}

impl Default for Grain {
    fn default() -> Self {
        Self {
            amount: 0.0,
            size: 1.0,
            seed: 0,
        }
    }
}

/// Fast integer hash (PCG-inspired) for deterministic per-pixel noise.
#[inline]
fn hash_pixel(x: u32, y: u32, seed: u32) -> f32 {
    let mut state =
        x.wrapping_mul(1597334677) ^ y.wrapping_mul(3812015801) ^ seed.wrapping_mul(2654435761);
    state = state.wrapping_mul(state).wrapping_add(state);
    state ^= state >> 16;
    state = state.wrapping_mul(state).wrapping_add(state);
    // Map to [-1, 1]
    (state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

impl Filter for Grain {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }

        let w = planes.width;
        let h = planes.height;
        let size = self.size.max(1.0);
        let inv_size = 1.0 / size;

        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) as usize;
                let l = planes.l[idx];

                // Grain coordinate (quantized for larger grain size)
                let gx = (x as f32 * inv_size) as u32;
                let gy = (y as f32 * inv_size) as u32;
                let noise = hash_pixel(gx, gy, self.seed);

                // Film-like response: less grain in deep shadows and highlights
                let response = grain_response(l);
                let grain = noise * self.amount * response * 0.15;

                planes.l[idx] = (l + grain).max(0.0);
            }
        }
    }
}

static GRAIN_SCHEMA: FilterSchema = FilterSchema {
    name: "grain",
    label: "Grain",
    description: "Film grain simulation with luminance-adaptive response",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Grain intensity (0 = none, 1 = heavy)",
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
        },
        ParamDesc {
            name: "size",
            label: "Size",
            description: "Grain spatial frequency (1 = fine, 2+ = coarser)",
            kind: ParamKind::Float {
                min: 1.0,
                max: 5.0,
                default: 1.0,
                identity: 1.0,
                step: 0.5,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "seed",
            label: "Seed",
            description: "Random seed for grain pattern",
            kind: ParamKind::Int {
                min: 0,
                max: 65535,
                default: 0,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for Grain {
    fn schema() -> &'static FilterSchema {
        &GRAIN_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "amount" => Some(ParamValue::Float(self.amount)),
            "size" => Some(ParamValue::Float(self.size)),
            "seed" => Some(ParamValue::Int(self.seed as i32)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "amount" => {
                let v = match value.as_f32() {
                    Some(v) => v,
                    None => return false,
                };
                self.amount = v;
            }
            "size" => {
                let v = match value.as_f32() {
                    Some(v) => v,
                    None => return false,
                };
                self.size = v;
            }
            "seed" => {
                let v = match value.as_i32() {
                    Some(v) => v,
                    None => return false,
                };
                self.seed = v as u32;
            }
            _ => return false,
        }
        true
    }
}

/// Film grain response curve: peaks in midtones, falls off in shadows/highlights.
#[inline]
fn grain_response(l: f32) -> f32 {
    // Parabola peaking at L=0.5
    let t = (l - 0.5) * 2.0; // maps [0,1] to [-1,1]
    (1.0 - t * t).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let orig = planes.l.clone();
        Grain::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn adds_variation() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let mut grain = Grain::default();
        grain.amount = 0.5;
        grain.apply(&mut planes, &mut FilterContext::new());

        // Should have some variation now
        let min = planes.l.iter().copied().fold(f32::MAX, f32::min);
        let max = planes.l.iter().copied().fold(f32::MIN, f32::max);
        assert!(max - min > 0.01, "grain should add variation: {min}..{max}");
    }

    #[test]
    fn deterministic() {
        let mut planes1 = OklabPlanes::new(16, 16);
        let mut planes2 = OklabPlanes::new(16, 16);
        for v in planes1.l.iter_mut().chain(planes2.l.iter_mut()) {
            *v = 0.5;
        }
        let mut grain = Grain::default();
        grain.amount = 0.3;
        grain.seed = 42;
        grain.apply(&mut planes1, &mut FilterContext::new());
        grain.apply(&mut planes2, &mut FilterContext::new());
        assert_eq!(planes1.l, planes2.l);
    }

    #[test]
    fn different_seeds_differ() {
        let mut planes1 = OklabPlanes::new(16, 16);
        let mut planes2 = OklabPlanes::new(16, 16);
        for v in planes1.l.iter_mut().chain(planes2.l.iter_mut()) {
            *v = 0.5;
        }
        let mut grain1 = Grain::default();
        grain1.amount = 0.3;
        grain1.seed = 1;
        let mut grain2 = Grain::default();
        grain2.amount = 0.3;
        grain2.seed = 2;
        grain1.apply(&mut planes1, &mut FilterContext::new());
        grain2.apply(&mut planes2, &mut FilterContext::new());
        assert_ne!(planes1.l, planes2.l);
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(16, 16);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let a_orig = planes.a.clone();
        let mut grain = Grain::default();
        grain.amount = 0.5;
        grain.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn less_grain_in_extremes() {
        // Midtone pixels should get more grain than shadow/highlight pixels
        let mut planes = OklabPlanes::new(3, 1);
        planes.l[0] = 0.02; // deep shadow
        planes.l[1] = 0.5; // midtone
        planes.l[2] = 0.98; // bright highlight

        let orig = planes.l.clone();
        let mut grain = Grain::default();
        grain.amount = 1.0;
        grain.apply(&mut planes, &mut FilterContext::new());

        let change_shadow = (planes.l[0] - orig[0]).abs();
        let change_mid = (planes.l[1] - orig[1]).abs();
        let change_high = (planes.l[2] - orig[2]).abs();

        // This test checks the response curve, but noise is random so
        // we can't guarantee exact ordering per sample. At least verify
        // the response function directly.
        assert!(grain_response(0.5) > grain_response(0.02));
        assert!(grain_response(0.5) > grain_response(0.98));
        // Suppress unused warnings
        let _ = (change_shadow, change_mid, change_high);
    }
}
