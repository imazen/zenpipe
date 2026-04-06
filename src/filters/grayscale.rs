use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Convert to grayscale by zeroing the chroma (a, b) channels.
///
/// In Oklab, grayscale means a=0, b=0 — the color is on the neutral axis.
/// This is the perceptually correct way to desaturate, unlike the sRGB
/// luma-coefficient hacks (BT.709, NTSC, flat average).
///
/// All three legacy grayscale modes (BT.709, NTSC, flat) produce the same
/// result in Oklab: zero chroma. The perceived luminance is already encoded
/// in the L channel, so there's no information loss.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Grayscale;

impl Filter for Grayscale {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        planes.a.fill(0.0);
        planes.b.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeros_chroma() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 32.0) * 0.01;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (32.0 - i as f32) * 0.01;
        }
        for v in &mut planes.l {
            *v = 0.5;
        }
        let l_orig = planes.l.clone();

        Grayscale.apply(&mut planes, &mut FilterContext::new());

        for &v in &planes.a {
            assert_eq!(v, 0.0);
        }
        for &v in &planes.b {
            assert_eq!(v, 0.0);
        }
        assert_eq!(planes.l, l_orig, "L should be unchanged");
    }
}
