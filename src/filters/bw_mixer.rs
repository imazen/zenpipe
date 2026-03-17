use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// B&W channel mixer — converts to grayscale with per-color luminance control.
///
/// Controls how much each color contributes to the final grayscale luminance.
/// This is the Oklab equivalent of Lightroom's B&W panel, which lets you make
/// reds brighter or darker in the B&W output, independently of blues, greens, etc.
///
/// 8 color ranges match the HSL adjust ranges: Red, Orange, Yellow, Green,
/// Aqua, Blue, Purple, Magenta. Each weight is a multiplier on that color's
/// luminance contribution (1.0 = neutral, >1 = brighter, <1 = darker).
///
/// The effect is proportional to chroma — neutral/gray pixels are unaffected
/// by the mixer weights (they're already colorless).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BwMixer {
    /// Per-color luminance weights. Order: R, O, Y, G, Aq, B, P, M.
    /// 1.0 = neutral. Range: typically 0.0 to 2.0.
    pub weights: [f32; 8],
}

impl Default for BwMixer {
    fn default() -> Self {
        Self { weights: [1.0; 8] }
    }
}

static BW_COLOR_LABELS: &[&str] = &[
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

static BW_MIXER_SCHEMA: FilterSchema = FilterSchema {
    name: "bw_mixer",
    label: "B&W Mixer",
    description: "Grayscale conversion with per-color luminance control",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "weights",
        label: "Channel Weights",
        description: "Per-color luminance weights (1 = neutral)",
        kind: ParamKind::FloatArray {
            len: 8,
            min: 0.0,
            max: 2.0,
            default: 1.0,
            labels: BW_COLOR_LABELS,
        },
        unit: "×",
        section: "Main",
        slider: SliderMapping::NotSlider,
    }],
};

impl Describe for BwMixer {
    fn schema() -> &'static FilterSchema {
        &BW_MIXER_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "weights" => Some(ParamValue::FloatArray(self.weights.to_vec())),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        if let ParamValue::FloatArray(ref arr) = value {
            if name == "weights" && arr.len() == 8 {
                self.weights.copy_from_slice(arr);
                return true;
            }
        }
        false
    }
}

/// Center hue angles in degrees for each range (matches HslAdjust).
const RANGE_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 280.0, 320.0];
const RANGE_HALF_WIDTH: f32 = 30.0;

#[inline]
fn range_weight(hue_deg: f32, center: f32) -> f32 {
    let mut diff = hue_deg - center;
    if diff > 180.0 {
        diff -= 360.0;
    }
    if diff < -180.0 {
        diff += 360.0;
    }
    let abs_diff = diff.abs();
    if abs_diff >= RANGE_HALF_WIDTH {
        return 0.0;
    }
    let t = abs_diff / RANGE_HALF_WIDTH;
    0.5 * (1.0 + (core::f32::consts::PI * t).cos())
}

impl BwMixer {
    fn is_identity(&self) -> bool {
        self.weights.iter().all(|&w| (w - 1.0).abs() < 1e-6)
    }
}

impl Filter for BwMixer {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let n = planes.pixel_count();

        for i in 0..n {
            let a = planes.a[i];
            let b = planes.b[i];
            let chroma = (a * a + b * b).sqrt();

            if chroma > 1e-5 && !self.is_identity() {
                let hue_rad = b.atan2(a);
                let mut hue_deg = hue_rad.to_degrees();
                if hue_deg < 0.0 {
                    hue_deg += 360.0;
                }

                // Compute weighted luminance multiplier from mixer weights
                let mut lum_scale = 0.0f32;
                let mut total_w = 0.0f32;
                for (r, &center) in RANGE_CENTERS.iter().enumerate() {
                    let w = range_weight(hue_deg, center);
                    if w > 1e-6 {
                        lum_scale += w * self.weights[r];
                        total_w += w;
                    }
                }

                if total_w > 1e-6 {
                    lum_scale /= total_w;

                    // Blend effect by chroma — more saturated pixels get more effect.
                    // 0.15 is the chroma at which the mixer reaches full influence.
                    // Typical sRGB skin tones have chroma ~0.05-0.08; saturated
                    // reds/blues reach ~0.25-0.3. Using 0.15 means the mixer is
                    // fully engaged for moderately saturated colors, giving strong
                    // hue-dependent B&W control without requiring extreme saturation.
                    let chroma_influence = (chroma / 0.15).min(1.0);
                    let effective_scale = 1.0 + (lum_scale - 1.0) * chroma_influence;

                    planes.l[i] = (planes.l[i] * effective_scale).max(0.0);
                }
            }

            // Always desaturate
            planes.a[i] = 0.0;
            planes.b[i] = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_produces_neutral_grayscale() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        planes.l[1] = 0.7;
        planes.a[1] = -0.05;
        planes.b[1] = 0.1;

        let l_orig = planes.l.clone();
        BwMixer::default().apply(&mut planes, &mut FilterContext::new());

        // L unchanged with default weights
        assert_eq!(planes.l, l_orig);
        // Chroma zeroed
        assert!(planes.a.iter().all(|&v| v == 0.0));
        assert!(planes.b.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn boosting_red_brightens_red_pixels() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15; // Red in Oklab: positive a
        planes.b[0] = 0.01;

        let mut mixer = BwMixer::default();
        mixer.weights[0] = 2.0; // boost red
        mixer.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > 0.5,
            "boosting red should brighten red pixel: {}",
            planes.l[0]
        );
        assert_eq!(planes.a[0], 0.0);
    }

    #[test]
    fn reducing_blue_darkens_blue_pixels() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        // Blue in Oklab: negative a, negative b
        planes.a[0] = -0.05;
        planes.b[0] = -0.15;

        let mut mixer = BwMixer::default();
        mixer.weights[5] = 0.3; // reduce blue
        mixer.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] < 0.5,
            "reducing blue should darken blue pixel: {}",
            planes.l[0]
        );
    }

    #[test]
    fn neutral_pixels_unaffected_by_weights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0; // neutral
        planes.b[0] = 0.0;

        let mut mixer = BwMixer::default();
        mixer.weights = [2.0, 0.5, 1.5, 0.3, 2.0, 0.1, 1.8, 0.4];
        mixer.apply(&mut planes, &mut FilterContext::new());

        assert!(
            (planes.l[0] - 0.5).abs() < 1e-5,
            "neutral should be unaffected: {}",
            planes.l[0]
        );
    }

    #[test]
    fn always_desaturates() {
        let mut planes = OklabPlanes::new(2, 2);
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = -0.05;
        }
        BwMixer::default().apply(&mut planes, &mut FilterContext::new());
        assert!(planes.a.iter().all(|&v| v == 0.0));
        assert!(planes.b.iter().all(|&v| v == 0.0));
    }
}
