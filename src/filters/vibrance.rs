use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

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

    fn apply(&self, planes: &mut OklabPlanes) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        const MAX_CHROMA: f32 = 0.4;
        let amount = self.amount;
        let protection = self.protection;

        let n = planes.pixel_count();
        for i in 0..n {
            let a = planes.a[i];
            let b = planes.b[i];
            let chroma = (a * a + b * b).sqrt();
            let normalized = (chroma / MAX_CHROMA).min(1.0);
            let protection_factor = (1.0 - normalized).powf(protection);
            let scale = 1.0 + amount * protection_factor;
            planes.a[i] = a * scale;
            planes.b[i] = b * scale;
        }
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
        .apply(&mut planes);
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
        .apply(&mut planes);

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
