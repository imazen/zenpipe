use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Automatic white balance: removes color cast by neutralizing mean a/b.
///
/// Measures the average Oklab a (green-red) and b (blue-yellow) offsets
/// and subtracts them, driving the average scene color toward neutral grey.
///
/// Different from AutoLevels' `remove_cast` option: this is the primary
/// operation, with saturation-weighted correction to avoid over-correcting
/// near-neutral scenes.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoWhiteBalance {
    /// Correction strength. 0.0 = off, 1.0 = full cast removal.
    pub strength: f32,
}

impl Default for AutoWhiteBalance {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

impl Filter for AutoWhiteBalance {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let a = ctx.analyze(planes);

        let cast_a = a.mean_a;
        let cast_b = a.mean_b;
        let cast_mag = (cast_a * cast_a + cast_b * cast_b).sqrt();

        // Skip if the image is already neutral
        if cast_mag < 0.005 {
            return;
        }

        // Saturation weight: don't over-correct low-saturation images
        // (they may be intentionally muted, and the cast estimate is noisy)
        let sat_weight = (a.chroma_energy / 0.08).clamp(0.3, 1.0);

        let scale = self.strength * sat_weight * 0.9; // 0.9: slight under-correction is safer

        if cast_a.abs() > 0.003 {
            simd::offset_plane(&mut planes.a, -cast_a * scale);
        }
        if cast_b.abs() > 0.003 {
            simd::offset_plane(&mut planes.b, -cast_b * scale);
        }
    }
}

static AUTO_WHITE_BALANCE_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_white_balance",
    label: "Auto White Balance",
    description: "Remove color cast by neutralizing average a/b chroma",
    group: FilterGroup::Auto,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Cast correction strength (0 = off, 1 = full)",
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

impl Describe for AutoWhiteBalance {
    fn schema() -> &'static FilterSchema {
        &AUTO_WHITE_BALANCE_SCHEMA
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
            *v = 0.1;
        }
        let orig = planes.a.clone();
        AutoWhiteBalance { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, orig);
    }

    #[test]
    fn removes_warm_cast() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        // Warm cast with some chroma variance (realistic)
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = 0.03 + (i as f32 / 200.0);
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = 0.08 + (i as f32 / 300.0);
        }
        let mean_b_before = planes.b.iter().sum::<f32>() / 100.0;
        AutoWhiteBalance { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let mean_b_after = planes.b.iter().sum::<f32>() / 100.0;
        assert!(
            mean_b_after.abs() < mean_b_before.abs() * 0.5,
            "warm cast should be reduced by at least half: {mean_b_before} -> {mean_b_after}"
        );
    }

    #[test]
    fn neutral_image_unchanged() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        // a and b near zero = neutral
        let orig_a = planes.a.clone();
        let orig_b = planes.b.clone();
        AutoWhiteBalance { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, orig_a);
        assert_eq!(planes.b, orig_b);
    }
}
