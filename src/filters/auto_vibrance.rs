use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Automatic per-hue vibrance boost.
///
/// Analyzes the image's per-hue-sector saturation and selectively boosts
/// muted color regions while leaving already-vivid areas alone. Different
/// from uniform Vibrance in that it adapts per hue sector — skin tones,
/// sky, and foliage are treated independently.
///
/// Uses 6 hue sectors (warm, yellow-green, green-cyan, cool, purple,
/// magenta-red) with per-sector target chroma. Sectors below target
/// get proportional boost; sectors at or above target are unchanged.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoVibrance {
    /// Boost strength. 0.0 = off, 1.0 = full adaptive boost.
    pub strength: f32,
}

impl Default for AutoVibrance {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

/// Hue sector boundaries and target chroma values.
/// 6 sectors in Oklab hue space (atan2(b, a)):
const NUM_SECTORS: usize = 6;
/// Sector boundaries in radians. Sectors wrap around at ±π.
const SECTOR_BOUNDS: [f32; 7] = [
    -core::f32::consts::PI, // -180° start
    -1.745,                 // -100° (cool/purple boundary)
    -0.873,                 // -50°  (purple/magenta boundary)
    0.0,                    // 0°    (magenta/warm boundary)
    1.047,                  // 60°   (warm/yellow-green boundary)
    2.094,                  // 120°  (yellow-green/green-cyan boundary)
    core::f32::consts::PI,  // 180°  (green-cyan/cool boundary = wrap)
];

/// Target chroma per sector. These represent "good" saturation for typical photos.
/// Sectors below these get boosted; sectors at or above are left alone.
const SECTOR_TARGETS: [f32; NUM_SECTORS] = [
    0.06, // cool (sky) — modest target, sky looks good without heavy saturation
    0.04, // purple — low target, purple is rare and easily oversaturated
    0.07, // magenta-red — skin often in this range, moderate target
    0.08, // warm (skin/earth) — slightly higher, warm colors benefit from saturation
    0.06, // yellow-green — foliage, moderate
    0.05, // green-cyan — moderate
];

fn hue_sector(a: f32, b: f32) -> usize {
    let hue = b.atan2(a);
    for i in 0..NUM_SECTORS {
        if hue < SECTOR_BOUNDS[i + 1] {
            return i;
        }
    }
    0 // wrap to first sector
}

impl Filter for AutoVibrance {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        // Pass 1: compute mean chroma per sector
        let mut sector_sum = [0.0f64; NUM_SECTORS];
        let mut sector_count = [0u32; NUM_SECTORS];

        for (&a, &b) in planes.a.iter().zip(planes.b.iter()) {
            let chroma = (a * a + b * b).sqrt();
            if chroma > 0.005 {
                let s = hue_sector(a, b);
                sector_sum[s] += chroma as f64;
                sector_count[s] += 1;
            }
        }

        let mut sector_mean = [0.0f32; NUM_SECTORS];
        for i in 0..NUM_SECTORS {
            if sector_count[i] > 0 {
                sector_mean[i] = (sector_sum[i] / sector_count[i] as f64) as f32;
            }
        }

        // Compute per-sector boost factor
        let mut sector_boost = [1.0f32; NUM_SECTORS];
        let mut any_boost = false;
        for i in 0..NUM_SECTORS {
            let deficit = SECTOR_TARGETS[i] - sector_mean[i];
            if deficit > 0.005 && sector_count[i] > 0 {
                // Boost proportional to deficit, capped to avoid over-saturation
                let boost_amount = (deficit / SECTOR_TARGETS[i]).min(1.0) * 0.5 * self.strength;
                sector_boost[i] = 1.0 + boost_amount;
                any_boost = true;
            }
        }

        if !any_boost {
            return;
        }

        // Pass 2: apply per-pixel boost based on hue sector
        let _ = ctx.analyze(planes); // ensure analysis is cached for downstream filters
        for (a_val, b_val) in planes.a.iter_mut().zip(planes.b.iter_mut()) {
            let a = *a_val;
            let b = *b_val;
            let chroma = (a * a + b * b).sqrt();
            if chroma < 0.005 {
                continue;
            }
            let s = hue_sector(a, b);
            let boost = sector_boost[s];
            if (boost - 1.0).abs() > 1e-6 {
                // Vibrance-style protection: reduce boost for already-saturated pixels
                let protection = (1.0 - (chroma / 0.3).min(1.0)).max(0.0);
                let effective = 1.0 + (boost - 1.0) * protection;
                *a_val = a * effective;
                *b_val = b * effective;
            }
        }
    }
}

static AUTO_VIBRANCE_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_vibrance",
    label: "Auto Vibrance",
    description: "Per-hue-sector adaptive saturation boost",
    group: FilterGroup::Auto,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Adaptive boost strength (0 = off, 1 = full)",
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

impl Describe for AutoVibrance {
    fn schema() -> &'static FilterSchema {
        &AUTO_VIBRANCE_SCHEMA
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = 0.03;
        }
        let orig_a = planes.a.clone();
        AutoVibrance { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, orig_a);
    }

    #[test]
    fn boosts_muted_colors() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        // Muted warm colors (low chroma in warm sector)
        for (i, (a, b)) in planes.a.iter_mut().zip(planes.b.iter_mut()).enumerate() {
            let t = i as f32 / 99.0;
            *a = 0.01 + t * 0.02; // small positive a
            *b = 0.01; // small positive b = warm hue
        }
        let chroma_before: f32 = planes
            .a
            .iter()
            .zip(planes.b.iter())
            .map(|(&a, &b)| (a * a + b * b).sqrt())
            .sum::<f32>()
            / 100.0;
        AutoVibrance { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after: f32 = planes
            .a
            .iter()
            .zip(planes.b.iter())
            .map(|(&a, &b)| (a * a + b * b).sqrt())
            .sum::<f32>()
            / 100.0;
        assert!(
            chroma_after > chroma_before * 1.05,
            "muted colors should be boosted: {chroma_before} -> {chroma_after}"
        );
    }
}
