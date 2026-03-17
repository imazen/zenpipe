use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Color inversion in Oklab space.
///
/// Inverts lightness (L' = 1.0 - L) and negates chroma (a' = -a, b' = -b).
/// This produces a perceptually correct negative — unlike sRGB inversion
/// (255 - v) which distorts perceived brightness relationships.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Invert;

impl Filter for Invert {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        // L' = 1.0 - L = -1 * L + 1.0
        simd::scale_plane(&mut planes.l, -1.0);
        simd::offset_plane(&mut planes.l, 1.0);
        // a' = -a, b' = -b
        simd::scale_plane(&mut planes.a, -1.0);
        simd::scale_plane(&mut planes.b, -1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_l() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let orig_l: Vec<f32> = planes.l.clone();
        Invert.apply(&mut planes, &mut FilterContext::new());
        for (i, &v) in planes.l.iter().enumerate() {
            let expected = 1.0 - orig_l[i];
            assert!(
                (v - expected).abs() < 1e-5,
                "L[{i}]: expected {expected}, got {v}"
            );
        }
    }

    #[test]
    fn negates_chroma() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 8.0) * 0.01;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (8.0 - i as f32) * 0.01;
        }
        let orig_a: Vec<f32> = planes.a.clone();
        let orig_b: Vec<f32> = planes.b.clone();
        Invert.apply(&mut planes, &mut FilterContext::new());
        for (i, &v) in planes.a.iter().enumerate() {
            assert!(
                (v + orig_a[i]).abs() < 1e-5,
                "a[{i}]: expected {}, got {v}",
                -orig_a[i]
            );
        }
        for (i, &v) in planes.b.iter().enumerate() {
            assert!(
                (v + orig_b[i]).abs() < 1e-5,
                "b[{i}]: expected {}, got {v}",
                -orig_b[i]
            );
        }
    }

    #[test]
    fn double_invert_is_identity() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 64.0;
        }
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 32.0) * 0.002;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (32.0 - i as f32) * 0.002;
        }
        let orig_l = planes.l.clone();
        let orig_a = planes.a.clone();
        let orig_b = planes.b.clone();

        Invert.apply(&mut planes, &mut FilterContext::new());
        Invert.apply(&mut planes, &mut FilterContext::new());

        for i in 0..planes.pixel_count() {
            assert!((planes.l[i] - orig_l[i]).abs() < 1e-4, "L[{i}] roundtrip");
            assert!((planes.a[i] - orig_a[i]).abs() < 1e-4, "a[{i}] roundtrip");
            assert!((planes.b[i] - orig_b[i]).abs() < 1e-4, "b[{i}] roundtrip");
        }
    }
}
