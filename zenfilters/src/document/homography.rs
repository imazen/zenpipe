//! DLT (Direct Linear Transform) for computing 3×3 homography matrices.
//!
//! Given 4 source points (detected document corners) and 4 destination points
//! (target rectangle), computes the projective transform matrix that maps
//! destination→source coordinates. This matrix feeds directly into
//! [`Warp::projective()`](crate::filters::Warp::projective).

/// Compute a 3×3 homography from 4 point correspondences.
///
/// Maps from destination to source coordinates (inverse mapping), matching
/// the convention used by [`Warp::projective()`](crate::filters::Warp::projective).
///
/// # Arguments
/// - `src`: 4 source points (detected document corners, in image coordinates)
/// - `dst`: 4 destination points (target rectangle corners)
///
/// # Returns
/// A 3×3 matrix in row-major order, or `None` if the system is degenerate.
///
/// # Algorithm
/// Uses DLT with Gaussian elimination on an 8×8 system.
/// For each correspondence (xₛ, yₛ) ↔ (x_d, y_d):
/// ```text
/// x_d·h₇·xₛ + x_d·h₈·yₛ + x_d - h₁·xₛ - h₂·yₛ - h₃ = 0
/// y_d·h₇·xₛ + y_d·h₈·yₛ + y_d - h₄·xₛ - h₅·yₛ - h₆ = 0
/// ```
/// We solve for h₁..h₈ with h₉ = 1.
pub fn compute_homography(src: &[(f32, f32); 4], dst: &[(f32, f32); 4]) -> Option<[f32; 9]> {
    // Build 8×9 augmented matrix [A | b] for Ah = b
    // We fix h[8] = 1.0 and solve the 8×8 system
    let mut a = [[0.0f64; 9]; 8];

    for i in 0..4 {
        // We want the matrix to map dst→src (inverse mapping for Warp).
        // So the equation is: src = H * dst
        // src_x = (h0*xd + h1*yd + h2) / (h6*xd + h7*yd + 1)
        let (xs, ys) = (src[i].0 as f64, src[i].1 as f64);
        let (xd, yd) = (dst[i].0 as f64, dst[i].1 as f64);

        let row0 = i * 2;
        let row1 = i * 2 + 1;

        // xs = (h0*xd + h1*yd + h2) / (h6*xd + h7*yd + 1)
        // → h0*xd + h1*yd + h2 - h6*xd*xs - h7*yd*xs = xs
        a[row0][0] = xd;
        a[row0][1] = yd;
        a[row0][2] = 1.0;
        a[row0][3] = 0.0;
        a[row0][4] = 0.0;
        a[row0][5] = 0.0;
        a[row0][6] = -xd * xs;
        a[row0][7] = -yd * xs;
        a[row0][8] = xs; // RHS

        // ys = (h3*xd + h4*yd + h5) / (h6*xd + h7*yd + 1)
        // → h3*xd + h4*yd + h5 - h6*xd*ys - h7*yd*ys = ys
        a[row1][0] = 0.0;
        a[row1][1] = 0.0;
        a[row1][2] = 0.0;
        a[row1][3] = xd;
        a[row1][4] = yd;
        a[row1][5] = 1.0;
        a[row1][6] = -xd * ys;
        a[row1][7] = -yd * ys;
        a[row1][8] = ys; // RHS
    }

    // Gaussian elimination with partial pivoting on 8×8 (using column 8 as RHS)
    for col in 0..8 {
        // Find pivot
        let mut max_val = a[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..8 {
            let val = a[row][col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None; // Degenerate configuration
        }

        // Swap rows
        if max_row != col {
            a.swap(col, max_row);
        }

        // Eliminate below
        let pivot = a[col][col];
        for row in (col + 1)..8 {
            let factor = a[row][col] / pivot;
            for c in col..9 {
                a[row][c] -= factor * a[col][c];
            }
        }
    }

    // Back substitution
    let mut h = [0.0f64; 8];
    for col in (0..8).rev() {
        let mut sum = a[col][8]; // RHS
        for c in (col + 1)..8 {
            sum -= a[col][c] * h[c];
        }
        if a[col][col].abs() < 1e-12 {
            return None;
        }
        h[col] = sum / a[col][col];
    }

    Some([
        h[0] as f32,
        h[1] as f32,
        h[2] as f32,
        h[3] as f32,
        h[4] as f32,
        h[5] as f32,
        h[6] as f32,
        h[7] as f32,
        1.0f32,
    ])
}

/// Convenience: compute homography for rectifying a document quad to a rectangle.
///
/// Given detected corners (TL, TR, BR, BL) and desired output dimensions,
/// produces the matrix for `Warp::projective()`.
pub fn rectify_quad(
    corners: &[(f32, f32); 4],
    output_width: f32,
    output_height: f32,
) -> Option<[f32; 9]> {
    let dst = [
        (0.0, 0.0),
        (output_width - 1.0, 0.0),
        (output_width - 1.0, output_height - 1.0),
        (0.0, output_height - 1.0),
    ];
    compute_homography(corners, &dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_mapping() {
        let points = [(0.0f32, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let h = compute_homography(&points, &points).unwrap();

        // Should be approximately identity: [1 0 0; 0 1 0; 0 0 1]
        assert!((h[0] - 1.0).abs() < 1e-4, "h[0]={}", h[0]);
        assert!(h[1].abs() < 1e-4, "h[1]={}", h[1]);
        assert!(h[2].abs() < 1e-4, "h[2]={}", h[2]);
        assert!(h[3].abs() < 1e-4, "h[3]={}", h[3]);
        assert!((h[4] - 1.0).abs() < 1e-4, "h[4]={}", h[4]);
        assert!(h[5].abs() < 1e-4, "h[5]={}", h[5]);
        assert!(h[6].abs() < 1e-4, "h[6]={}", h[6]);
        assert!(h[7].abs() < 1e-4, "h[7]={}", h[7]);
    }

    #[test]
    fn simple_translation() {
        // Shift right by 10, down by 20
        let src = [
            (10.0f32, 20.0),
            (110.0, 20.0),
            (110.0, 120.0),
            (10.0, 120.0),
        ];
        let dst = [(0.0f32, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let h = compute_homography(&src, &dst).unwrap();

        // Map dst (0,0) → should give src (10,20)
        // src_x = h[0]*0 + h[1]*0 + h[2] = h[2] ≈ 10
        // src_y = h[3]*0 + h[4]*0 + h[5] = h[5] ≈ 20
        assert!(
            (h[2] - 10.0).abs() < 0.1,
            "translation x: expected 10, got {}",
            h[2]
        );
        assert!(
            (h[5] - 20.0).abs() < 0.1,
            "translation y: expected 20, got {}",
            h[5]
        );
    }

    #[test]
    fn perspective_transform() {
        // Trapezoid → rectangle (simulated perspective correction)
        let src = [
            (20.0f32, 10.0), // TL (shifted inward at top)
            (80.0, 10.0),    // TR
            (100.0, 90.0),   // BR (wider at bottom)
            (0.0, 90.0),     // BL
        ];
        let dst = [(0.0f32, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let h = compute_homography(&src, &dst);
        assert!(h.is_some(), "perspective transform should be solvable");

        let h = h.unwrap();
        // Verify: mapping dst corner (0,0) should give approximately src corner (20,10)
        let w = h[6] * 0.0 + h[7] * 0.0 + 1.0;
        let sx = (h[0] * 0.0 + h[1] * 0.0 + h[2]) / w;
        let sy = (h[3] * 0.0 + h[4] * 0.0 + h[5]) / w;
        assert!(
            (sx - 20.0).abs() < 0.5 && (sy - 10.0).abs() < 0.5,
            "TL corner: expected (20,10), got ({sx:.1},{sy:.1})"
        );
    }

    #[test]
    fn rectify_quad_produces_valid_matrix() {
        let corners = [(10.0f32, 5.0), (90.0, 8.0), (95.0, 92.0), (8.0, 88.0)];
        let h = rectify_quad(&corners, 100.0, 100.0);
        assert!(h.is_some());
    }

    #[test]
    fn degenerate_returns_none() {
        // All points at the same location
        let points = [(50.0f32, 50.0); 4];
        let h = compute_homography(&points, &points);
        assert!(h.is_none(), "degenerate points should return None");
    }
}
