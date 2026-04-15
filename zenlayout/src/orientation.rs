//! Orientation (D4 dihedral group), EXIF mapping, and coordinate transforms.

use crate::constraint::{Rect, Size};

/// Image orientation as an element of the D4 dihedral group.
///
/// Every orientation decomposes into a rotation (0°/90°/180°/270° clockwise)
/// optionally followed by a horizontal flip. All 8 EXIF orientations map
/// one-to-one to these variants.
///
/// The composition rule matches the D4 Cayley table verified against
/// zenjpeg's `coeff_transform.rs`.
///
/// ```text
///     EXIF orientations and their transforms:
///
///     1: Identity    2: FlipH       3: Rotate180   4: FlipV
///     ┌───┐          ┌───┐          ┌───┐          ┌───┐
///     │ F │          │ Ꟊ │          │   │          │   │
///     │   │          │   │          │ Ꟊ │          │ F │
///     └───┘          └───┘          └───┘          └───┘
///
///     5: Transpose   6: Rotate90    7: Transverse  8: Rotate270
///     ┌────┐         ┌────┐         ┌────┐         ┌────┐
///     │ F  │         │  F │         │  Ꟊ │         │ Ꟊ  │
///     └────┘         └────┘         └────┘         └────┘
/// ```
///
/// # Decomposition
///
/// ```text
/// | Orientation | = Rotation | + FlipH? | Swaps axes? |
/// |-------------|------------|----------|-------------|
/// | Identity    | 0°         | no       | no          |
/// | FlipH       | 0°         | yes      | no          |
/// | Rotate180   | 180°       | no       | no          |
/// | FlipV       | 180°       | yes      | no          |
/// | Transpose   | 90° CW     | yes      | yes         |
/// | Rotate90    | 90° CW     | no       | yes         |
/// | Transverse  | 270° CW    | yes      | yes         |
/// | Rotate270   | 270° CW    | no       | yes         |
/// ```
#[non_exhaustive]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// No transformation. EXIF 1.
    #[default]
    Identity,
    /// Horizontal flip. EXIF 2.
    FlipH,
    /// 180° rotation. EXIF 3.
    Rotate180,
    /// Vertical flip (= Rotate180 + FlipH). EXIF 4.
    FlipV,
    /// Transpose: reflect over main diagonal (= Rotate90 + FlipH). EXIF 5. Swaps axes.
    Transpose,
    /// 90° clockwise rotation. EXIF 6. Swaps axes.
    Rotate90,
    /// Transverse: reflect over anti-diagonal (= Rotate270 + FlipH). EXIF 7. Swaps axes.
    Transverse,
    /// 270° clockwise rotation (90° counter-clockwise). EXIF 8. Swaps axes.
    Rotate270,
}

impl Orientation {
    /// Decompose into `(rotation_quarters, flip)` for composition math.
    ///
    /// `rotation_quarters` is 0-3 (number of 90° CW steps).
    /// `flip` is true if a horizontal flip follows the rotation.
    const fn decompose(self) -> (u8, bool) {
        match self {
            Self::Identity => (0, false),
            Self::FlipH => (0, true),
            Self::Rotate90 => (1, false),
            Self::Transpose => (1, true),
            Self::Rotate180 => (2, false),
            Self::FlipV => (2, true),
            Self::Rotate270 => (3, false),
            Self::Transverse => (3, true),
        }
    }

    /// Reconstruct from `(rotation_quarters & 3, flip)`.
    const fn from_rotation_flip(rotation: u8, flip: bool) -> Self {
        match (rotation & 3, flip) {
            (0, false) => Self::Identity,
            (0, true) => Self::FlipH,
            (1, false) => Self::Rotate90,
            (1, true) => Self::Transpose,
            (2, false) => Self::Rotate180,
            (2, true) => Self::FlipV,
            (3, false) => Self::Rotate270,
            (3, true) => Self::Transverse,
            _ => unreachable!(),
        }
    }

    /// Create from EXIF orientation tag (1-8). Returns `None` for invalid values.
    pub const fn from_exif(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Identity),
            2 => Some(Self::FlipH),
            3 => Some(Self::Rotate180),
            4 => Some(Self::FlipV),
            5 => Some(Self::Transpose),
            6 => Some(Self::Rotate90),
            7 => Some(Self::Transverse),
            8 => Some(Self::Rotate270),
            _ => None,
        }
    }

    /// Convert to EXIF orientation tag (1-8).
    pub const fn to_exif(self) -> u8 {
        match self {
            Self::Identity => 1,
            Self::FlipH => 2,
            Self::Rotate180 => 3,
            Self::FlipV => 4,
            Self::Transpose => 5,
            Self::Rotate90 => 6,
            Self::Transverse => 7,
            Self::Rotate270 => 8,
        }
    }

    /// Whether this is the identity transformation.
    pub const fn is_identity(self) -> bool {
        matches!(self, Self::Identity)
    }

    /// Whether this orientation swaps width and height.
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }

    /// Compose two orientations: apply `self` first, then `other`.
    ///
    /// Alias: [`then`](Self::then) reads more naturally in chains.
    ///
    /// This follows the D4 group multiplication rule verified against
    /// the Cayley table in zenjpeg's `coeff_transform.rs`.
    pub const fn compose(self, other: Self) -> Self {
        let (r1, f1) = self.decompose();
        let (r2, f2) = other.decompose();
        if !f1 {
            Self::from_rotation_flip((r1 + r2) & 3, f2)
        } else {
            Self::from_rotation_flip(r1.wrapping_sub(r2) & 3, !f2)
        }
    }

    /// Alias for [`compose`](Self::compose). Reads naturally in chains:
    /// `Rotate90.then(FlipH)` = apply Rotate90 first, then FlipH.
    pub const fn then(self, other: Self) -> Self {
        self.compose(other)
    }

    /// The inverse orientation: `self.compose(self.inverse()) == Identity`.
    pub const fn inverse(self) -> Self {
        let (r, f) = self.decompose();
        if !f {
            Self::from_rotation_flip((4 - r) & 3, false)
        } else {
            // Flips are self-inverse, but rotation direction reverses under flip
            self
        }
    }

    /// Transform source dimensions to display dimensions.
    pub const fn transform_dimensions(self, w: u32, h: u32) -> Size {
        if self.swaps_axes() {
            Size::new(h, w)
        } else {
            Size::new(w, h)
        }
    }

    /// Transform a rectangle from display coordinates back to source coordinates.
    ///
    /// Given a rect in post-orientation (display) space and the source image
    /// dimensions, returns the corresponding rect in pre-orientation (source) space.
    pub fn transform_rect_to_source(self, rect: Rect, source_w: u32, source_h: u32) -> Rect {
        let (rx, ry, rw, rh) = (rect.x, rect.y, rect.width, rect.height);
        let (sw, sh) = (source_w, source_h);

        match self {
            Self::Identity => Rect::new(rx, ry, rw, rh),
            Self::FlipH => Rect::new(sw - rx - rw, ry, rw, rh),
            Self::Rotate90 => Rect::new(ry, sh - rx - rw, rh, rw),
            Self::Transpose => Rect::new(ry, rx, rh, rw),
            Self::Rotate180 => Rect::new(sw - rx - rw, sh - ry - rh, rw, rh),
            Self::FlipV => Rect::new(rx, sh - ry - rh, rw, rh),
            Self::Rotate270 => Rect::new(sw - ry - rh, rx, rh, rw),
            Self::Transverse => Rect::new(sw - ry - rh, sh - rx - rw, rh, rw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All 8 elements of the D4 group, indexed by EXIF value - 1.
    const ALL: [Orientation; 8] = [
        Orientation::Identity,
        Orientation::FlipH,
        Orientation::Rotate180,
        Orientation::FlipV,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];

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
    fn exif_mapping_matches_spec() {
        // Verified against zenjpeg exif.rs:168-180
        assert_eq!(Orientation::from_exif(1).unwrap(), Orientation::Identity);
        assert_eq!(Orientation::from_exif(2).unwrap(), Orientation::FlipH);
        assert_eq!(Orientation::from_exif(3).unwrap(), Orientation::Rotate180);
        assert_eq!(Orientation::from_exif(4).unwrap(), Orientation::FlipV);
        assert_eq!(Orientation::from_exif(5).unwrap(), Orientation::Transpose);
        assert_eq!(Orientation::from_exif(6).unwrap(), Orientation::Rotate90);
        assert_eq!(Orientation::from_exif(7).unwrap(), Orientation::Transverse);
        assert_eq!(Orientation::from_exif(8).unwrap(), Orientation::Rotate270);
    }

    #[test]
    fn identity_properties() {
        assert!(Orientation::Identity.is_identity());
        assert!(!Orientation::FlipH.is_identity());
        assert!(!Orientation::Rotate90.is_identity());
    }

    #[test]
    fn swaps_axes() {
        assert!(!Orientation::Identity.swaps_axes());
        assert!(!Orientation::FlipH.swaps_axes());
        assert!(!Orientation::Rotate180.swaps_axes());
        assert!(!Orientation::FlipV.swaps_axes());
        assert!(Orientation::Transpose.swaps_axes());
        assert!(Orientation::Rotate90.swaps_axes());
        assert!(Orientation::Transverse.swaps_axes());
        assert!(Orientation::Rotate270.swaps_axes());
    }

    #[test]
    fn transform_dimensions() {
        use crate::constraint::Size;
        assert_eq!(
            Orientation::Identity.transform_dimensions(100, 200),
            Size::new(100, 200)
        );
        assert_eq!(
            Orientation::FlipH.transform_dimensions(100, 200),
            Size::new(100, 200)
        );
        assert_eq!(
            Orientation::Rotate180.transform_dimensions(100, 200),
            Size::new(100, 200)
        );
        assert_eq!(
            Orientation::FlipV.transform_dimensions(100, 200),
            Size::new(100, 200)
        );
        assert_eq!(
            Orientation::Transpose.transform_dimensions(100, 200),
            Size::new(200, 100)
        );
        assert_eq!(
            Orientation::Rotate90.transform_dimensions(100, 200),
            Size::new(200, 100)
        );
        assert_eq!(
            Orientation::Transverse.transform_dimensions(100, 200),
            Size::new(200, 100)
        );
        assert_eq!(
            Orientation::Rotate270.transform_dimensions(100, 200),
            Size::new(200, 100)
        );
    }

    /// Verify the full D4 Cayley table against zenjpeg's coeff_transform.rs.
    ///
    /// The Cayley table from zenjpeg uses indices:
    /// 0=None, 1=FlipH, 2=FlipV, 3=Transpose, 4=Rotate90, 5=Rotate180, 6=Rotate270, 7=Transverse
    ///
    /// Our EXIF-ordered ALL array uses:
    /// 0=Identity, 1=FlipH, 2=Rotate180, 3=FlipV, 4=Transpose, 5=Rotate90, 6=Transverse, 7=Rotate270
    ///
    /// So we need a mapping between the two index orders.
    #[test]
    fn cayley_table() {
        // zenjpeg Cayley table (from coeff_transform.rs:130-140)
        // Index order: None=0, FlipH=1, FlipV=2, Transpose=3, Rot90=4, Rot180=5, Rot270=6, Transverse=7
        #[rustfmt::skip]
        const CAYLEY: [[usize; 8]; 8] = [
            [0,1,2,3,4,5,6,7], // None
            [1,0,5,6,7,2,3,4], // FlipH
            [2,5,0,4,3,1,7,6], // FlipV (note: not at EXIF index 2)
            [3,4,6,0,1,7,2,5], // Transpose
            [4,3,7,2,5,6,0,1], // Rotate90
            [5,2,1,7,6,0,4,3], // Rotate180
            [6,7,3,1,0,4,5,2], // Rotate270
            [7,6,4,5,2,3,1,0], // Transverse
        ];

        // zenjpeg index order to Orientation
        let zj_to_orient = [
            Orientation::Identity,   // 0 = None
            Orientation::FlipH,      // 1 = FlipH
            Orientation::FlipV,      // 2 = FlipV
            Orientation::Transpose,  // 3 = Transpose
            Orientation::Rotate90,   // 4 = Rotate90
            Orientation::Rotate180,  // 5 = Rotate180
            Orientation::Rotate270,  // 6 = Rotate270
            Orientation::Transverse, // 7 = Transverse
        ];

        for (i, row) in CAYLEY.iter().enumerate() {
            for (j, &expected_idx) in row.iter().enumerate() {
                let a = zj_to_orient[i];
                let b = zj_to_orient[j];
                let expected = zj_to_orient[expected_idx];
                let got = a.compose(b);
                assert_eq!(
                    got, expected,
                    "Cayley mismatch: {a:?}.compose({b:?}) = {got:?}, expected {expected:?}"
                );
            }
        }
    }

    #[test]
    fn inverse_all() {
        let all = ALL;
        for &o in &all {
            let inv = o.inverse();
            assert_eq!(
                o.compose(inv),
                Orientation::Identity,
                "{o:?}.compose({inv:?}) should be Identity"
            );
            assert_eq!(
                inv.compose(o),
                Orientation::Identity,
                "{inv:?}.compose({o:?}) should be Identity"
            );
        }
    }

    #[test]
    fn associativity() {
        let all = ALL;
        for &a in &all {
            for &b in &all {
                for &c in &all {
                    let ab_c = a.compose(b).compose(c);
                    let a_bc = a.compose(b.compose(c));
                    assert_eq!(
                        ab_c, a_bc,
                        "associativity failed: ({a:?}*{b:?})*{c:?} != {a:?}*({b:?}*{c:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn identity_is_neutral() {
        let id = Orientation::Identity;
        for &o in &ALL {
            assert_eq!(id.compose(o), o);
            assert_eq!(o.compose(id), o);
        }
    }

    #[test]
    fn transform_rect_identity() {
        let rect = Rect::new(10, 20, 30, 40);
        let result = Orientation::Identity.transform_rect_to_source(rect, 100, 200);
        assert_eq!(result, rect);
    }

    #[test]
    fn transform_rect_full_image() {
        // Full image rect should map to full source rect for all orientations
        for &o in &ALL {
            let d = o.transform_dimensions(100, 200);
            let display_rect = Rect::new(0, 0, d.width, d.height);
            let source_rect = o.transform_rect_to_source(display_rect, 100, 200);
            assert_eq!(
                (source_rect.x, source_rect.y),
                (0, 0),
                "full image rect origin for {o:?}"
            );
            assert_eq!(
                (source_rect.width, source_rect.height),
                (100, 200),
                "full image rect dims for {o:?}"
            );
        }
    }

    #[test]
    fn transform_rect_1x1_at_corners() {
        // Test 1x1 pixel rects at all 4 corners of a 4x3 image
        let (sw, sh) = (4u32, 3u32);
        let corners = [
            (0, 0), // top-left
            (3, 0), // top-right (sw-1, 0)
            (0, 2), // bottom-left (0, sh-1)
            (3, 2), // bottom-right (sw-1, sh-1)
        ];

        for &o in &ALL {
            let d = o.transform_dimensions(sw, sh);
            for &(sx, sy) in &corners {
                // Forward-map this source pixel to display coords
                let (dx, dy) = forward_map_point(o, sx, sy, sw, sh);
                assert!(
                    dx < d.width && dy < d.height,
                    "forward mapped ({sx},{sy}) to ({dx},{dy}) but display is {}x{} for {o:?}",
                    d.width,
                    d.height
                );

                // Now transform_rect_to_source should give us back the source pixel
                let display_rect = Rect::new(dx, dy, 1, 1);
                let source_rect = o.transform_rect_to_source(display_rect, sw, sh);
                assert_eq!(
                    (
                        source_rect.x,
                        source_rect.y,
                        source_rect.width,
                        source_rect.height
                    ),
                    (sx, sy, 1, 1),
                    "round-trip failed for source ({sx},{sy}) display ({dx},{dy}) orient {o:?}"
                );
            }
        }
    }

    #[test]
    fn transform_rect_brute_force_4x3() {
        // Test every single-pixel rect in a 4x3 image
        let (sw, sh) = (4u32, 3u32);
        for &o in &ALL {
            let d = o.transform_dimensions(sw, sh);
            for sx in 0..sw {
                for sy in 0..sh {
                    let (dx, dy) = forward_map_point(o, sx, sy, sw, sh);
                    let display_rect = Rect::new(dx, dy, 1, 1);
                    let source_rect = o.transform_rect_to_source(display_rect, sw, sh);
                    assert_eq!(
                        (source_rect.x, source_rect.y),
                        (sx, sy),
                        "pixel ({sx},{sy}) via {o:?}: display ({dx},{dy}) in {}x{}, got back ({},{})",
                        d.width,
                        d.height,
                        source_rect.x,
                        source_rect.y
                    );
                }
            }
        }
    }

    #[test]
    fn transform_rect_multi_pixel() {
        // Test a 2x2 rect in a 4x3 image for all orientations
        let (sw, sh) = (4u32, 3u32);
        let rect = Rect::new(1, 1, 2, 2);

        // For identity: source rect is (1,1,2,2), display rect is same
        let result = Orientation::Identity.transform_rect_to_source(rect, sw, sh);
        assert_eq!(result, rect);

        // For all orientations: forward-map all 4 pixels in the 2x2 block,
        // find bounding box in display, that should be what maps back
        for &o in &ALL {
            // Forward-map corner pixels
            let (dx0, dy0) = forward_map_point(o, rect.x, rect.y, sw, sh);
            let (dx1, dy1) = forward_map_point(o, rect.x + rect.width - 1, rect.y, sw, sh);
            let (dx2, dy2) = forward_map_point(o, rect.x, rect.y + rect.height - 1, sw, sh);
            let (dx3, dy3) =
                forward_map_point(o, rect.x + rect.width - 1, rect.y + rect.height - 1, sw, sh);

            let min_x = dx0.min(dx1).min(dx2).min(dx3);
            let min_y = dy0.min(dy1).min(dy2).min(dy3);
            let max_x = dx0.max(dx1).max(dx2).max(dx3);
            let max_y = dy0.max(dy1).max(dy2).max(dy3);

            let display_rect = Rect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);

            let source_rect = o.transform_rect_to_source(display_rect, sw, sh);
            assert_eq!(
                source_rect, rect,
                "multi-pixel rect {rect:?} via {o:?}: display {display_rect:?} → source {source_rect:?}"
            );
        }
    }

    /// Forward-map a source pixel to display coordinates.
    /// Verified against zenjpeg coeff_transform.rs:89-97.
    fn forward_map_point(o: Orientation, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
        match o {
            Orientation::Identity => (x, y),
            Orientation::FlipH => (w - 1 - x, y),
            Orientation::Rotate90 => (h - 1 - y, x),
            Orientation::Transpose => (y, x),
            Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
            Orientation::FlipV => (x, h - 1 - y),
            Orientation::Rotate270 => (y, w - 1 - x),
            Orientation::Transverse => (h - 1 - y, w - 1 - x),
        }
    }
}
