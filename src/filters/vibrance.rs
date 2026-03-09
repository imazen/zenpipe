use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Smart saturation that protects already-saturated colors.
///
/// Boosts chroma of low-saturation pixels more than high-saturation ones,
/// preventing skin tone and sky clipping. The protection curve is:
/// `scale = 1 + amount * (1 - chroma / max_chroma)^protection`
pub struct Vibrance {
    /// Vibrance amount. 0.0 = no change, 1.0 = full boost.
    pub amount: f32,
    /// Protection exponent. Higher = more protection for saturated colors.
    /// Default: 2.0.
    pub protection: f32,
}

impl Default for Vibrance {
    fn default() -> Self {
        Self {
            amount: 0.0,
            protection: 2.0,
        }
    }
}

impl Filter for Vibrance {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        simd::vibrance(&mut planes.a, &mut planes.b, self.amount, self.protection);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.a[0] = 0.1;
        planes.b[0] = -0.05;
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        Vibrance {
            amount: 0.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn low_saturation_gets_more_boost() {
        let mut planes = OklabPlanes::new(2, 1);
        // Pixel 0: low saturation
        planes.a[0] = 0.02;
        planes.b[0] = 0.01;
        // Pixel 1: high saturation
        planes.a[1] = 0.3;
        planes.b[1] = 0.2;

        let c0_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c1_before = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        Vibrance {
            amount: 0.5,
            protection: 2.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let c0_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c1_after = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        let boost0 = c0_after / c0_before;
        let boost1 = c1_after / c1_before;
        assert!(
            boost0 > boost1,
            "low-sat pixel should get bigger boost: {boost0} vs {boost1}"
        );
    }
}
