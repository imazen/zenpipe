use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// White point adjustment on Oklab L channel.
///
/// Scales the L range so that `level` maps to L=1.0.
/// For SDR, default is 1.0 (no change). Values < 1.0 brighten highlights;
/// values > 1.0 extend the dynamic range.
pub struct WhitePoint {
    /// White point level. 1.0 = no change.
    pub level: f32,
}

impl Filter for WhitePoint {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes) {
        if (self.level - 1.0).abs() < 1e-6 {
            return;
        }
        let inv = 1.0 / self.level.max(0.01);
        for v in &mut planes.l {
            *v *= inv;
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
        WhitePoint { level: 1.0 }.apply(&mut planes);
        assert_eq!(planes.l, original);
    }

    #[test]
    fn below_one_brightens() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        WhitePoint { level: 0.8 }.apply(&mut planes);
        assert!(planes.l[0] > 0.5);
    }
}
