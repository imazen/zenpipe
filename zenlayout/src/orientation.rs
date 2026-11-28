//! Orientation (D4 dihedral group), EXIF mapping, and checked coordinate transforms.
//!
//! [`Orientation`] is re-exported directly from [`zenpixels::Orientation`] so every
//! crate in the stack shares one canonical D4 enum. Geometry helpers over [`Rect`]
//! and [`Size`] are provided by [`OrientationExt`].

use crate::constraint::{Rect, Size};

pub use zenpixels::Orientation;

/// Geometry helpers for [`zenpixels::Orientation`] over [`Size`] and half-open [`Rect`].
pub trait OrientationExt: Copy {
    /// Compose two orientations: apply `self` first, then `other`.
    ///
    /// Equivalent to [`Orientation::then`].
    fn compose(self, other: Orientation) -> Orientation;

    /// Transform source dimensions to display dimensions.
    fn transform_dimensions(self, w: u32, h: u32) -> Size;

    /// Transform a rectangle from display coordinates back to source coordinates.
    ///
    /// Saturates on out-of-bounds input so adversarial inputs never panic.
    /// Use [`transform_rect_to_source_checked`](Self::transform_rect_to_source_checked)
    /// when out-of-bounds rectangles must be rejected instead of clamped.
    fn transform_rect_to_source(self, rect: Rect, source_w: u32, source_h: u32) -> Rect;

    /// Strictly transform a half-open display rectangle `[x, x + w) × [y, y + h)`
    /// back into source coordinates `[0, source_w) × [0, source_h)`.
    ///
    /// Returns `None` if `rect` overflows `u32` or leaves the display bounds.
    fn transform_rect_to_source_checked(
        self,
        rect: Rect,
        source_w: u32,
        source_h: u32,
    ) -> Option<Rect>;

    /// Strictly transform a half-open source rectangle `[x, x + w) × [y, y + h)`
    /// forward into display coordinates `[0, disp_w) × [0, disp_h)`.
    ///
    /// Returns `None` if `rect` overflows `u32` or leaves the source bounds.
    fn transform_rect_from_source_checked(
        self,
        rect: Rect,
        source_w: u32,
        source_h: u32,
    ) -> Option<Rect>;
}

impl OrientationExt for Orientation {
    #[inline]
    fn compose(self, other: Orientation) -> Orientation {
        self.then(other)
    }

    #[inline]
    fn transform_dimensions(self, w: u32, h: u32) -> Size {
        let (ow, oh) = self.output_dimensions(w, h);
        Size::new(ow, oh)
    }

    fn transform_rect_to_source(self, rect: Rect, source_w: u32, source_h: u32) -> Rect {
        let (rx, ry, rw, rh) = (rect.x, rect.y, rect.width, rect.height);
        let (sw, sh) = (source_w, source_h);

        // Helper: sw - rx - rw, never underflows.
        let sub2 = |a: u32, b: u32, c: u32| a.saturating_sub(b).saturating_sub(c);

        match self {
            Orientation::Identity => Rect::new(rx, ry, rw, rh),
            Orientation::FlipH => Rect::new(sub2(sw, rx, rw), ry, rw, rh),
            Orientation::Rotate90 => Rect::new(ry, sub2(sh, rx, rw), rh, rw),
            Orientation::Transpose => Rect::new(ry, rx, rh, rw),
            Orientation::Rotate180 => Rect::new(sub2(sw, rx, rw), sub2(sh, ry, rh), rw, rh),
            Orientation::FlipV => Rect::new(rx, sub2(sh, ry, rh), rw, rh),
            Orientation::Rotate270 => Rect::new(sub2(sw, ry, rh), rx, rh, rw),
            Orientation::Transverse => Rect::new(sub2(sw, ry, rh), sub2(sh, rx, rw), rh, rw),
            _ => Rect::new(rx, ry, rw, rh),
        }
    }

    fn transform_rect_to_source_checked(
        self,
        rect: Rect,
        source_w: u32,
        source_h: u32,
    ) -> Option<Rect> {
        let disp = self.transform_dimensions(source_w, source_h);
        let rx_end = rect.x.checked_add(rect.width)?;
        let ry_end = rect.y.checked_add(rect.height)?;
        if rx_end > disp.width || ry_end > disp.height {
            return None;
        }
        let (rx, ry, rw, rh) = (rect.x, rect.y, rect.width, rect.height);
        let (sw, sh) = (source_w, source_h);
        let checked_sub2 = |a: u32, b: u32, c: u32| a.checked_sub(b)?.checked_sub(c);
        Some(match self {
            Orientation::Identity => Rect::new(rx, ry, rw, rh),
            Orientation::FlipH => Rect::new(checked_sub2(sw, rx, rw)?, ry, rw, rh),
            Orientation::Rotate90 => Rect::new(ry, checked_sub2(sh, rx, rw)?, rh, rw),
            Orientation::Transpose => Rect::new(ry, rx, rh, rw),
            Orientation::Rotate180 => {
                Rect::new(checked_sub2(sw, rx, rw)?, checked_sub2(sh, ry, rh)?, rw, rh)
            }
            Orientation::FlipV => Rect::new(rx, checked_sub2(sh, ry, rh)?, rw, rh),
            Orientation::Rotate270 => Rect::new(checked_sub2(sw, ry, rh)?, rx, rh, rw),
            Orientation::Transverse => {
                Rect::new(checked_sub2(sw, ry, rh)?, checked_sub2(sh, rx, rw)?, rh, rw)
            }
            _ => return None,
        })
    }

    fn transform_rect_from_source_checked(
        self,
        rect: Rect,
        source_w: u32,
        source_h: u32,
    ) -> Option<Rect> {
        // Forward-mapping a source rect under `self` equals inverse-mapping from
        // source under `self.inverse()`, whose output space is `transform_dimensions(sw, sh)`.
        let disp = self.transform_dimensions(source_w, source_h);
        self.inverse()
            .transform_rect_to_source_checked(rect, disp.width, disp.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Orientation; 8] = Orientation::ALL;

    #[test]
    fn exif_round_trip() {
        for v in 1..=8u8 {
            let o = Orientation::from_exif(v).unwrap();
            assert_eq!(o.to_exif(), v, "round-trip failed for EXIF {v}");
        }
    }

    #[test]
    fn exif_invalid() {
        assert!(Orientation::from_exif(0).is_none());
        assert!(Orientation::from_exif(9).is_none());
        assert!(Orientation::from_exif(255).is_none());
    }

    #[test]
    fn transform_dimensions_all() {
        assert_eq!(Orientation::Identity.transform_dimensions(100, 200), Size::new(100, 200));
        assert_eq!(Orientation::FlipH.transform_dimensions(100, 200), Size::new(100, 200));
        assert_eq!(Orientation::Rotate180.transform_dimensions(100, 200), Size::new(100, 200));
        assert_eq!(Orientation::FlipV.transform_dimensions(100, 200), Size::new(100, 200));
        assert_eq!(Orientation::Transpose.transform_dimensions(100, 200), Size::new(200, 100));
        assert_eq!(Orientation::Rotate90.transform_dimensions(100, 200), Size::new(200, 100));
        assert_eq!(Orientation::Transverse.transform_dimensions(100, 200), Size::new(200, 100));
        assert_eq!(Orientation::Rotate270.transform_dimensions(100, 200), Size::new(200, 100));
    }

    #[test]
    fn cayley_table() {
        #[rustfmt::skip]
        const CAYLEY: [[usize; 8]; 8] = [
            [0,1,2,3,4,5,6,7],
            [1,0,5,6,7,2,3,4],
            [2,5,0,4,3,1,7,6],
            [3,4,6,0,1,7,2,5],
            [4,3,7,2,5,6,0,1],
            [5,2,1,7,6,0,4,3],
            [6,7,3,1,0,4,5,2],
            [7,6,4,5,2,3,1,0],
        ];
        let zj_to_orient = [
            Orientation::Identity,
            Orientation::FlipH,
            Orientation::FlipV,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
            Orientation::Transverse,
        ];
        for (i, row) in CAYLEY.iter().enumerate() {
            for (j, &expected_idx) in row.iter().enumerate() {
                assert_eq!(
                    zj_to_orient[i].compose(zj_to_orient[j]),
                    zj_to_orient[expected_idx]
                );
            }
        }
    }

    #[test]
    fn checked_rect_roundtrip_and_oob() {
        let (sw, sh) = (17u32, 11u32);
        let src_rect = Rect::new(3, 2, 7, 5);
        for &o in &ALL {
            let disp = o
                .transform_rect_from_source_checked(src_rect, sw, sh)
                .unwrap();
            let back = o.transform_rect_to_source_checked(disp, sw, sh).unwrap();
            assert_eq!(back, src_rect, "checked round-trip failed for {o:?}");
            // Verify single-pixel exactness matches zenpixels::Orientation::map_pixel_coordinates
            let disp_size = o.transform_dimensions(sw, sh);
            for sx in 0..4u32 {
                for sy in 0..3u32 {
                    let (dx, dy) = o.map_pixel_coordinates((sx, sy), (4, 3));
                    let got = o
                        .transform_rect_to_source_checked(Rect::new(dx, dy, 1, 1), 4, 3)
                        .unwrap();
                    assert_eq!(got, Rect::new(sx, sy, 1, 1));
                }
            }
            // Out-of-bounds rect must return None in checked mode and never panic in saturating mode
            let oob = Rect::new(disp_size.width, 0, 1, 1);
            assert!(o.transform_rect_to_source_checked(oob, sw, sh).is_none());
            let _ = o.transform_rect_to_source(oob, sw, sh);
        }
    }
}
