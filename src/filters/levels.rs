use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Levels adjustment: input/output range remapping with gamma correction.
///
/// Maps the input luminance range `[in_black, in_white]` to the output range
/// `[out_black, out_white]` with a gamma curve controlling the midpoint.
///
/// This is the classic Photoshop/Lightroom Levels dialog:
/// - `in_black` / `in_white`: clip input range (shadows/highlights clipping)
/// - `gamma`: midtone adjustment (1.0 = linear, <1 = darken mids, >1 = brighten mids)
/// - `out_black` / `out_white`: remap output range (reduce contrast)
///
/// Processing order:
/// 1. Remap input: `t = (L - in_black) / (in_white - in_black)`, clamp [0,1]
/// 2. Apply gamma: `t' = t^(1/gamma)`
/// 3. Remap output: `L' = out_black + t' * (out_white - out_black)`
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Levels {
    /// Input black point. Pixels at or below this become black.
    /// Range: 0.0–1.0. Default: 0.0.
    pub in_black: f32,
    /// Input white point. Pixels at or above this become white.
    /// Range: 0.0–1.0. Default: 1.0.
    pub in_white: f32,
    /// Midtone gamma. 1.0 = linear, >1 = brighten midtones, <1 = darken.
    /// Range: 0.1–10.0. Default: 1.0.
    pub gamma: f32,
    /// Output black point. Minimum output luminance.
    /// Range: 0.0–1.0. Default: 0.0.
    pub out_black: f32,
    /// Output white point. Maximum output luminance.
    /// Range: 0.0–1.0. Default: 1.0.
    pub out_white: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            in_black: 0.0,
            in_white: 1.0,
            gamma: 1.0,
            out_black: 0.0,
            out_white: 1.0,
        }
    }
}

impl Levels {
    fn is_identity(&self) -> bool {
        self.in_black.abs() < 1e-6
            && (self.in_white - 1.0).abs() < 1e-6
            && (self.gamma - 1.0).abs() < 1e-6
            && self.out_black.abs() < 1e-6
            && (self.out_white - 1.0).abs() < 1e-6
    }
}

impl Filter for Levels {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let in_range = (self.in_white - self.in_black).max(1e-6);
        let inv_in_range = 1.0 / in_range;
        let out_range = self.out_white - self.out_black;
        let inv_gamma = 1.0 / self.gamma.max(0.01);

        // For gamma=1 (common case), skip the pow entirely
        if (self.gamma - 1.0).abs() < 1e-6 {
            // Linear remap: L' = out_black + clamp((L - in_black) / in_range) * out_range
            let bp = self.in_black;
            let combined_scale = out_range * inv_in_range;
            let combined_offset = self.out_black - bp * combined_scale;
            simd::scale_plane(&mut planes.l, combined_scale);
            simd::offset_plane(&mut planes.l, combined_offset);
            // Clamp
            for v in &mut planes.l {
                *v = v.clamp(self.out_black, self.out_white);
            }
        } else {
            // Full gamma remap
            for v in &mut planes.l {
                let t = ((*v - self.in_black) * inv_in_range).clamp(0.0, 1.0);
                let t_gamma = t.powf(inv_gamma);
                *v = self.out_black + t_gamma * out_range;
            }
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
        let original = planes.l.clone();
        Levels::default().apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-5, "identity failed: {a} vs {b}");
        }
    }

    #[test]
    fn input_clipping() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.1, 0.3, 0.7, 0.9];

        let mut levels = Levels::default();
        levels.in_black = 0.2;
        levels.in_white = 0.8;
        levels.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] < 0.01,
            "below in_black should clip to 0: {}",
            planes.l[0]
        );
        assert!(
            (planes.l[3] - 1.0).abs() < 0.02,
            "above in_white should clip to 1: {}",
            planes.l[3]
        );
    }

    #[test]
    fn gamma_brightens_midtones() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5; // midtone

        let mut levels = Levels::default();
        levels.gamma = 2.0; // brighten mids
        levels.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > 0.5,
            "gamma>1 should brighten midtones: {}",
            planes.l[0]
        );
    }

    #[test]
    fn output_range_limits() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l = vec![0.0, 1.0];

        let mut levels = Levels::default();
        levels.out_black = 0.2;
        levels.out_white = 0.8;
        levels.apply(&mut planes, &mut FilterContext::new());

        assert!(
            (planes.l[0] - 0.2).abs() < 1e-4,
            "black should map to out_black: {}",
            planes.l[0]
        );
        assert!(
            (planes.l[1] - 0.8).abs() < 1e-4,
            "white should map to out_white: {}",
            planes.l[1]
        );
    }
}
