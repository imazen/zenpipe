use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::filters::guided_filter::guided_filter_plane;
use crate::planes::OklabPlanes;

/// Zone-based luminance adjustment with edge-aware masking.
///
/// The tone equalizer divides the luminance range into 9 zones (one per
/// photographic stop from −8 EV to 0 EV) and applies independent exposure
/// compensation to each zone. A guided filter creates an edge-preserving
/// mask so adjustments don't cause halos at high-contrast boundaries.
///
/// This is the most flexible local tone adjustment tool — more precise than
/// Highlights/Shadows, more targeted than LocalToneMap. It allows raising
/// shadows by +2 EV while compressing highlights by −1 EV, all without halos.
///
/// Equivalent to darktable's Tone Equalizer module (Aurélien Pierre, 2019).
///
/// # Zones
///
/// `zones[0]` = darkest (−8 EV, near-black), `zones[8]` = brightest (0 EV, white).
/// Each value is an exposure compensation in stops: positive lifts, negative darkens.
/// Default: all zeros (identity).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ToneEqualizer {
    /// Exposure compensation per zone, in stops. 9 zones from dark to bright.
    /// zones[0] = −8 EV (deepest shadows), zones[8] = 0 EV (highlights).
    pub zones: [f32; 9],
    /// Guided filter sigma — controls the spatial scale of the luminance mask.
    /// Larger values = smoother transitions, less local contrast preservation.
    /// Typical: 1–10% of image diagonal. Default: relative to image size.
    pub smoothing: f32,
    /// Edge preservation strength (guided filter eps). Smaller = sharper edges
    /// in the mask. Default: 0.01 (strong edge preservation).
    pub edge_preservation: f32,
}

impl Default for ToneEqualizer {
    fn default() -> Self {
        Self {
            zones: [0.0; 9],
            smoothing: 0.0, // 0 = auto-size based on image dimensions
            edge_preservation: 0.01,
        }
    }
}

impl ToneEqualizer {
    fn is_identity(&self) -> bool {
        self.zones.iter().all(|z| z.abs() < 1e-6)
    }

    /// Build a 256-entry LUT mapping L [0,1] to exposure compensation factor.
    ///
    /// Each zone is a Gaussian window centered at its EV position. The zone
    /// weights overlap smoothly and are normalized so they sum to a constant.
    fn build_compensation_lut(&self) -> Vec<f32> {
        let lut_size = crate::LUT_SIZE;
        let lut_max = crate::LUT_MAX as f32;
        let mut lut = vec![1.0f32; lut_size];

        const ZONE_CENTERS: [f32; 9] = [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
        const ZONE_WIDTH: f32 = 0.15;
        const INV_2_WIDTH_SQ: f32 = 1.0 / (2.0 * 0.15 * 0.15);

        for i in 0..lut_size {
            let l = i as f32 / lut_max;
            let mut total_weight = 0.0f32;
            let mut total_comp = 0.0f32;

            for (z, &center) in ZONE_CENTERS.iter().enumerate() {
                let d = l - center;
                let weight = (-d * d * INV_2_WIDTH_SQ).exp();
                // Convert stops to Oklab L factor: 2^(stops/3) for cube-root domain
                let factor = 2.0f32.powf(self.zones[z] / 3.0);
                total_weight += weight;
                total_comp += weight * factor;
            }

            lut[i] = if total_weight > 1e-8 {
                total_comp / total_weight
            } else {
                1.0
            };
        }

        lut
    }
}

impl Filter for ToneEqualizer {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        let sigma = if self.smoothing > 0.0 {
            self.smoothing
        } else {
            // Auto: 5% of image diagonal
            let diag = ((width * width + height * height) as f32).sqrt();
            diag * 0.05
        };
        (sigma * 3.0).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let w = planes.width;
        let h = planes.height;
        let n = (w as usize) * (h as usize);

        let sigma = if self.smoothing > 0.0 {
            self.smoothing
        } else {
            let diag = ((w * w + h * h) as f32).sqrt();
            diag * 0.05
        };

        // 1. Create edge-preserving luminance guide
        let mut guide = ctx.take_f32(n);
        guided_filter_plane(
            &planes.l,
            &planes.l,
            &mut guide,
            w,
            h,
            sigma,
            self.edge_preservation,
            ctx,
        );

        // 2. Build zone compensation LUT
        let lut = self.build_compensation_lut();

        // 3. Apply: L' = L * lut[guide_value]
        // The guide determines which zone each pixel belongs to (edge-aware),
        // and the LUT provides the smooth compensation factor.
        let lut_max = crate::LUT_MAX;
        let scale = lut_max as f32;
        for i in 0..n {
            let guide_l = guide[i].clamp(0.0, 1.0);
            let x = guide_l * scale;
            let idx = x as usize;
            let frac = x - idx as f32;

            let factor = if idx < lut_max {
                lut[idx] * (1.0 - frac) + lut[idx + 1] * frac
            } else {
                lut[lut_max]
            };

            planes.l[i] = (planes.l[i] * factor).max(0.0);
        }

        ctx.return_f32(guide);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_zones_is_identity() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        ToneEqualizer::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn shadow_lift_brightens_darks() {
        let mut planes = OklabPlanes::new(16, 16);
        for v in &mut planes.l {
            *v = 0.1; // dark pixel (near zone 1)
        }
        let original_l = planes.l[0];

        let mut te = ToneEqualizer::default();
        te.zones[0] = 2.0; // lift deepest shadows by 2 stops
        te.zones[1] = 2.0;
        te.smoothing = 3.0; // small for test image
        te.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > original_l * 1.1,
            "shadows should be lifted: {} → {}",
            original_l,
            planes.l[0]
        );
    }

    #[test]
    fn highlight_compression_darkens_brights() {
        let mut planes = OklabPlanes::new(16, 16);
        for v in &mut planes.l {
            *v = 0.9; // bright pixel
        }
        let original_l = planes.l[0];

        let mut te = ToneEqualizer::default();
        te.zones[7] = -1.0; // compress near-highlights
        te.zones[8] = -1.0; // compress highlights
        te.smoothing = 3.0;
        te.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] < original_l,
            "highlights should be compressed: {} → {}",
            original_l,
            planes.l[0]
        );
    }
}
