use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// White point adjustment on Oklab L channel.
///
/// Scales the L range so that `level` maps to L=1.0.
/// For SDR, default is 1.0 (no change). Values < 1.0 brighten highlights;
/// values > 1.0 extend the dynamic range.
///
/// When `headroom > 0.0`, values above the white point are soft-clipped
/// using an asymptotic rolloff instead of hard scaling. This preserves
/// highlight detail by compressing super-white values into a headroom
/// band above the white point, rather than clipping them.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WhitePoint {
    /// White point level. 1.0 = no change.
    pub level: f32,
    /// Headroom fraction for soft-clip rolloff above the white point.
    /// 0.0 = hard linear scale (default, current behavior).
    /// Typical values: 0.05–0.2. The headroom band extends from
    /// `level` to `level * (1 + headroom)`.
    pub headroom: f32,
}

impl Default for WhitePoint {
    fn default() -> Self {
        Self {
            level: 1.0,
            headroom: 0.0,
        }
    }
}

/// Soft asymptotic rolloff for values above `white_point`.
///
/// Below `white_point`, values pass through unchanged.
/// Above, they are compressed into a headroom band using an exponential
/// curve that approaches `white_point + headroom` asymptotically.
#[inline]
fn soft_clip(l: f32, white_point: f32, headroom_fraction: f32) -> f32 {
    if l <= 0.0 {
        0.0
    } else if l <= white_point {
        l
    } else {
        let headroom = white_point * headroom_fraction;
        let excess = l - white_point;
        let k = 3.0 / headroom.max(0.01);
        white_point + headroom * (1.0 - (-excess * k).exp())
    }
}

impl Filter for WhitePoint {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if (self.level - 1.0).abs() < 1e-6 && self.headroom <= 0.0 {
            return;
        }

        if self.headroom <= 1e-6 {
            // Fast path: simple linear scale (original behavior).
            let inv = 1.0 / self.level.max(0.01);
            simd::scale_plane(&mut planes.l, inv);
        } else {
            // Soft-clip path: scale then apply asymptotic rolloff.
            let inv = 1.0 / self.level.max(0.01);
            let wp = 1.0; // After scaling, white point is at 1.0
            let headroom = self.headroom;
            for val in &mut planes.l {
                let scaled = *val * inv;
                *val = soft_clip(scaled, wp, headroom);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.5;
        planes.l[1] = 0.9;
        let original = planes.l.clone();
        WhitePoint {
            level: 1.0,
            headroom: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn below_one_brightens() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        WhitePoint {
            level: 0.8,
            headroom: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] > 0.5);
    }

    #[test]
    fn default_has_zero_headroom() {
        let wp = WhitePoint::default();
        assert_eq!(wp.level, 1.0);
        assert_eq!(wp.headroom, 0.0);
    }

    // --- headroom tests ---

    #[test]
    fn headroom_below_white_point_passes_through() {
        // Values at or below the white point should be unaffected by headroom.
        let mut planes = OklabPlanes::new(3, 1);
        planes.l[0] = 0.0;
        planes.l[1] = 0.5;
        planes.l[2] = 0.8; // 0.8 * (1/0.8) = 1.0, right at white point

        let mut planes_no_hr = planes.clone();

        WhitePoint {
            level: 0.8,
            headroom: 0.1,
        }
        .apply(&mut planes, &mut FilterContext::new());

        WhitePoint {
            level: 0.8,
            headroom: 0.0,
        }
        .apply(&mut planes_no_hr, &mut FilterContext::new());

        // Values that scale to <= 1.0 should match the linear path
        // l[0] = 0.0 stays 0.0
        assert!((planes.l[0] - 0.0).abs() < 1e-6);
        // l[1] = 0.5 * (1/0.8) = 0.625, below white point, identical
        assert!((planes.l[1] - planes_no_hr.l[1]).abs() < 1e-6);
    }

    #[test]
    fn headroom_compresses_super_whites() {
        // A value above the white point should be compressed, not linearly scaled.
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 1.2; // Will scale to 1.2/0.8 = 1.5, well above 1.0

        WhitePoint {
            level: 0.8,
            headroom: 0.1,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let result = planes.l[0];
        // Should be above 1.0 (the white point) but below the linear scale (1.5)
        assert!(result > 1.0, "should be above white point: {result}");
        assert!(result < 1.5, "should be compressed below linear: {result}");
        // Should be at or below 1.0 + headroom_band = 1.0 + 1.0*0.1 = 1.1
        assert!(
            result <= 1.1 + 1e-6,
            "should stay within headroom band: {result}"
        );
    }

    #[test]
    fn headroom_monotonic() {
        // Soft clip must be monotonically increasing: higher input → higher output.
        let mut planes = OklabPlanes::new(5, 1);
        for (i, v) in [0.85, 0.90, 1.0, 1.2, 1.5].iter().enumerate() {
            planes.l[i] = *v;
        }
        WhitePoint {
            level: 0.8,
            headroom: 0.15,
        }
        .apply(&mut planes, &mut FilterContext::new());

        for i in 1..5 {
            assert!(
                planes.l[i] >= planes.l[i - 1],
                "not monotonic at index {i}: {} < {}",
                planes.l[i],
                planes.l[i - 1]
            );
        }
    }

    #[test]
    fn headroom_approaches_limit() {
        // Very large input should approach white_point + headroom_band but not exceed it.
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 10.0; // extreme value

        WhitePoint {
            level: 0.8,
            headroom: 0.1,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let result = planes.l[0];
        let limit = 1.0 + 1.0 * 0.1; // white_point=1.0 after scale, headroom_band=0.1
        assert!(
            result < limit + 1e-4,
            "should not exceed headroom limit {limit}: got {result}"
        );
        assert!(
            result > limit - 0.01,
            "extreme value should be near limit: got {result}, limit {limit}"
        );
    }

    #[test]
    fn soft_clip_unit() {
        // Direct unit tests for the soft_clip function.
        // Zero passes through
        assert_eq!(soft_clip(0.0, 1.0, 0.1), 0.0);

        // Below white point passes through
        assert_eq!(soft_clip(0.5, 1.0, 0.1), 0.5);
        assert_eq!(soft_clip(1.0, 1.0, 0.1), 1.0);

        // Above white point is compressed
        let clipped = soft_clip(1.5, 1.0, 0.1);
        assert!(clipped > 1.0);
        assert!(clipped < 1.5);

        // Negative passes through as 0
        assert_eq!(soft_clip(-0.5, 1.0, 0.1), 0.0);
    }

    #[test]
    fn soft_clip_continuous_at_white_point() {
        // The function should be continuous at the white point boundary.
        let wp = 1.0;
        let hr = 0.1;
        let at_wp = soft_clip(wp, wp, hr);
        let just_above = soft_clip(wp + 1e-6, wp, hr);
        assert!(
            (at_wp - just_above).abs() < 1e-4,
            "discontinuity at white point: {at_wp} vs {just_above}"
        );
    }
}
