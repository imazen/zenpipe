use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Adaptive sharpening: noise-gated unsharp mask on L channel.
///
/// Unlike uniform sharpening (which amplifies noise in flat areas like
/// sky and skin), this measures local texture energy and only sharpens
/// where there's actual detail to enhance.
///
/// Algorithm:
/// 1. Extract detail: `detail = L - blur(L, sigma)`
/// 2. Estimate local energy: `energy = blur(detail², sigma * 3)`
/// 3. Compute noise gate: `gate = sqrt(energy) / (sqrt(energy) + noise_floor)`
/// 4. Apply: `L' = L + amount * detail * gate`
///
/// The gate smoothly ramps from 0 (flat areas, noise) to 1 (textured areas).
/// This is what phone cameras should do but don't — they sharpen everything
/// uniformly, making sky grain and compression artifacts worse.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AdaptiveSharpen {
    /// Sharpening strength. Typical: 0.3-1.5.
    pub amount: f32,
    /// Detail extraction sigma. Small = fine detail. Typical: 0.8-2.0.
    pub sigma: f32,
    /// Noise floor in L standard deviation units. Detail below this
    /// is considered noise and won't be sharpened. Typical: 0.003-0.01.
    pub noise_floor: f32,
}

impl Default for AdaptiveSharpen {
    fn default() -> Self {
        Self {
            amount: 0.0,
            sigma: 1.0,
            noise_floor: 0.005,
        }
    }
}

impl Filter for AdaptiveSharpen {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;

        // 1. Fine blur → detail extraction
        let kernel_detail = GaussianKernel::new(self.sigma);
        let mut blurred = ctx.take_f32(pc);
        gaussian_blur_plane(&planes.l, &mut blurred, w, h, &kernel_detail, ctx);

        // detail = L - blurred (high-frequency content)
        let mut detail = ctx.take_f32(pc);
        for i in 0..pc {
            detail[i] = planes.l[i] - blurred[i];
        }
        ctx.return_f32(blurred);

        // 2. Local energy = blur(detail²)
        let mut detail_sq = ctx.take_f32(pc);
        for i in 0..pc {
            detail_sq[i] = detail[i] * detail[i];
        }

        let kernel_energy = GaussianKernel::new(self.sigma * 3.0);
        let mut energy = ctx.take_f32(pc);
        gaussian_blur_plane(&detail_sq, &mut energy, w, h, &kernel_energy, ctx);
        ctx.return_f32(detail_sq);

        // 3+4. Gated sharpening: L' = L + amount * detail * gate
        // gate = sqrt(energy) / (sqrt(energy) + noise_floor)
        let nf = self.noise_floor;
        let amount = self.amount;
        let mut dst = ctx.take_f32(pc);
        for i in 0..pc {
            let e = energy[i].max(0.0).sqrt();
            let gate = e / (e + nf);
            dst[i] = (planes.l[i] + amount * detail[i] * gate).max(0.0);
        }

        ctx.return_f32(energy);
        ctx.return_f32(detail);
        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 1024.0).sin().abs();
        }
        let original = planes.l.clone();
        AdaptiveSharpen {
            amount: 0.0,
            sigma: 1.0,
            noise_floor: 0.005,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn sharpens_textured_areas() {
        let mut planes = OklabPlanes::new(64, 64);
        // Checkerboard = high texture energy everywhere
        for y in 0..64 {
            for x in 0..64 {
                let i = y * 64 + x;
                planes.l[i] = if (x + y) % 2 == 0 { 0.6 } else { 0.4 };
            }
        }
        let std_before = std_dev(&planes.l);
        AdaptiveSharpen {
            amount: 1.0,
            sigma: 1.0,
            noise_floor: 0.001,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let std_after = std_dev(&planes.l);
        assert!(
            std_after > std_before,
            "textured area should be sharpened: {std_before} -> {std_after}"
        );
    }

    #[test]
    fn skips_flat_areas() {
        let mut planes = OklabPlanes::new(64, 64);
        // Uniform with tiny noise (below noise_floor)
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.5 + (i as f32 * 0.0001).sin() * 0.001;
        }
        let original = planes.l.clone();
        AdaptiveSharpen {
            amount: 2.0,
            sigma: 1.0,
            noise_floor: 0.01, // well above the noise level
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Should barely change — noise gate blocks sharpening
        let max_diff = planes
            .l
            .iter()
            .zip(original.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 0.01,
            "flat areas should be barely affected: max_diff={max_diff}"
        );
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 0.4 } else { 0.6 };
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        AdaptiveSharpen {
            amount: 1.0,
            sigma: 1.0,
            noise_floor: 0.001,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    fn std_dev(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        let variance =
            data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32;
        variance.sqrt()
    }
}
