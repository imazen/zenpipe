//!
//! Given LSD line segments, generates candidate quadrilaterals and scores them
//! by edge response, inside/outside contrast, and geometric plausibility.
//! Returns the best-scoring document boundary.

extern crate alloc;

use super::lsd::LineSegment;
use crate::prelude::*;

/// A detected document quadrilateral with quality score.
#[derive(Clone, Debug)]
pub struct DocumentQuad {
    /// Four corners in order: TL, TR, BR, BL.
    pub corners: [(f32, f32); 4],
    /// Quality score (higher = better). Combines edge response, contrast, geometry.
    pub score: f32,
}

/// Minimum segment length as a fraction of the image diagonal.
const MIN_SEGMENT_FRAC: f32 = 0.05;

/// Maximum angle deviation from horizontal/vertical to be classified (degrees).
const ANGLE_CLASSIFY_TOLERANCE: f32 = 25.0;

/// Find the best document quadrilateral from detected line segments.
///
/// # Arguments
/// - `segments`: Line segments from LSD
/// - `l_plane`: The L channel (for contrast scoring)
/// - `grad_mag`: Gradient magnitude (for edge response scoring)
/// - `width`, `height`: Image dimensions
///
/// Returns the best scoring quad, or `None` if no plausible document found.
pub fn find_document_quad(
    segments: &[LineSegment],
    l_plane: &[f32],
    grad_mag: &[f32],
    width: u32,
    height: u32,
) -> Option<DocumentQuad> {
    let w = width as f32;
    let h = height as f32;
    let diag = (w * w + h * h).sqrt();
    let min_len = diag * MIN_SEGMENT_FRAC;

    // Filter and classify segments
    let classify_tol_rad = ANGLE_CLASSIFY_TOLERANCE * core::f32::consts::PI / 180.0;

    let mut horizontals: Vec<&LineSegment> = Vec::new();
    let mut verticals: Vec<&LineSegment> = Vec::new();

    for seg in segments {
        if seg.length < min_len {
            continue;
        }

        let abs_angle = seg.angle.abs();
        // Horizontal: angle near 0 or ±π
        if abs_angle < classify_tol_rad || (core::f32::consts::PI - abs_angle) < classify_tol_rad {
            horizontals.push(seg);
        }
        // Vertical: angle near ±π/2
        else if (abs_angle - core::f32::consts::FRAC_PI_2).abs() < classify_tol_rad {
            verticals.push(seg);
        }
    }

    if horizontals.len() < 2 || verticals.len() < 2 {
        return None;
    }

    // Split horizontals into top/bottom candidates by y position
    let mid_y = h * 0.5;
    let top_h: Vec<_> = horizontals
        .iter()
        .filter(|s| midpoint_y(s) < mid_y)
        .collect();
    let bot_h: Vec<_> = horizontals
        .iter()
        .filter(|s| midpoint_y(s) >= mid_y)
        .collect();

    // Split verticals into left/right by x position
    let mid_x = w * 0.5;
    let left_v: Vec<_> = verticals.iter().filter(|s| midpoint_x(s) < mid_x).collect();
    let right_v: Vec<_> = verticals
        .iter()
        .filter(|s| midpoint_x(s) >= mid_x)
        .collect();

    if top_h.is_empty() || bot_h.is_empty() || left_v.is_empty() || right_v.is_empty() {
        return None;
    }

    // Limit combinatorics: take top N from each group (sorted by significance/length)
    let max_per_group = 5;
    let top_h = &top_h[..top_h.len().min(max_per_group)];
    let bot_h = &bot_h[..bot_h.len().min(max_per_group)];
    let left_v = &left_v[..left_v.len().min(max_per_group)];
    let right_v = &right_v[..right_v.len().min(max_per_group)];

    let mut best: Option<DocumentQuad> = None;

    for &top in top_h {
        for &bot in bot_h {
            for &left in left_v {
                for &right in right_v {
                    // Compute 4 intersection points
                    let tl = intersect_lines(top, left);
                    let tr = intersect_lines(top, right);
                    let br = intersect_lines(bot, right);
                    let bl = intersect_lines(bot, left);

                    let (tl, tr, br, bl) = match (tl, tr, br, bl) {
                        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                        _ => continue,
                    };

                    // Check all corners are within image bounds (with margin)
                    let margin = -diag * 0.05;
                    let max_x = w - margin;
                    let max_y = h - margin;
                    if !in_bounds(tl, margin, margin, max_x, max_y)
                        || !in_bounds(tr, margin, margin, max_x, max_y)
                        || !in_bounds(br, margin, margin, max_x, max_y)
                        || !in_bounds(bl, margin, margin, max_x, max_y)
                    {
                        continue;
                    }

                    let corners = [tl, tr, br, bl];

                    // Check convexity
                    if !is_convex(&corners) {
                        continue;
                    }

                    let score = score_quad(&corners, l_plane, grad_mag, width, height);

                    if score > best.as_ref().map_or(0.0, |b| b.score) {
                        best = Some(DocumentQuad { corners, score });
                    }
                }
            }
        }
    }

    best
}

/// Score a candidate quadrilateral.
///
/// Combines:
/// 1. Edge response: mean gradient magnitude along the quad perimeter
/// 2. Inside/outside contrast: difference in mean L between inside and outside
/// 3. Area ratio: penalize very small or very large quads
/// 4. Angle quality: penalize acute or obtuse interior angles
pub fn score_quad(
    corners: &[(f32, f32); 4],
    l_plane: &[f32],
    grad_mag: &[f32],
    width: u32,
    height: u32,
) -> f32 {
    let w = width as usize;
    let h = height as usize;
    let img_area = (w * h) as f32;

    // 1. Edge response: sample gradient magnitude along each edge
    let edge_score = {
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for i in 0..4 {
            let (x0, y0) = corners[i];
            let (x1, y1) = corners[(i + 1) % 4];
            let steps = ((x1 - x0).abs().max((y1 - y0).abs()) as u32).max(1);
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let x = (x0 + (x1 - x0) * t) as usize;
                let y = (y0 + (y1 - y0) * t) as usize;
                if x < w && y < h {
                    sum += grad_mag[y * w + x];
                    count += 1;
                }
            }
        }
        if count > 0 { sum / count as f32 } else { 0.0 }
    };

    // 2. Inside/outside contrast (Tropin et al. 2020 approach)
    let contrast_score = {
        let mut inside_sum = 0.0f64;
        let mut inside_count = 0u32;
        let mut outside_sum = 0.0f64;
        let mut outside_count = 0u32;

        // Sample a grid of points and classify as inside/outside quad
        let step = ((w.max(h)) / 50).max(1);
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                let l = l_plane[y * w + x] as f64;
                if point_in_quad(x as f32, y as f32, corners) {
                    inside_sum += l;
                    inside_count += 1;
                } else {
                    outside_sum += l;
                    outside_count += 1;
                }
            }
        }

        if inside_count > 0 && outside_count > 0 {
            let inside_mean = inside_sum / inside_count as f64;
            let outside_mean = outside_sum / outside_count as f64;
            (inside_mean - outside_mean).abs() as f32
        } else {
            0.0
        }
    };

    // 3. Area ratio
    let quad_area = polygon_area(corners);
    let area_ratio = quad_area / img_area;
    let area_score = if area_ratio < 0.05 || area_ratio > 0.98 {
        0.0 // too small or too large
    } else {
        // Prefer medium-large quads (sweet spot around 30-80% of image)
        1.0 - (area_ratio - 0.55).abs() * 0.5
    };

    // 4. Angle quality: interior angles should be 60-120°
    let angle_score = {
        let mut min_angle = f32::MAX;
        for i in 0..4 {
            let p0 = corners[(i + 3) % 4];
            let p1 = corners[i];
            let p2 = corners[(i + 1) % 4];
            let a = interior_angle(p0, p1, p2);
            min_angle = min_angle.min(a);
        }
        let min_deg = min_angle.to_degrees();
        if min_deg < 45.0 {
            0.0
        } else if min_deg < 60.0 {
            (min_deg - 45.0) / 15.0
        } else {
            1.0
        }
    };

    // Weighted combination
    edge_score * 2.0 + contrast_score * 3.0 + area_score * 1.0 + angle_score * 1.0
}

// ─── Geometry helpers ───────────────────────────────────────────────

fn midpoint_x(seg: &LineSegment) -> f32 {
    (seg.x1 + seg.x2) * 0.5
}

fn midpoint_y(seg: &LineSegment) -> f32 {
    (seg.y1 + seg.y2) * 0.5
}

/// Intersect two infinite lines defined by line segments.
/// Returns the intersection point, or None if parallel.
fn intersect_lines(a: &LineSegment, b: &LineSegment) -> Option<(f32, f32)> {
    let (x1, y1, x2, y2) = (a.x1, a.y1, a.x2, a.y2);
    let (x3, y3, x4, y4) = (b.x1, b.y1, b.x2, b.y2);

    let denom = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4);
    if denom.abs() < 1e-6 {
        return None; // Parallel
    }

    let t = ((x1 - x3) * (y3 - y4) - (y1 - y3) * (x3 - x4)) / denom;
    let ix = x1 + t * (x2 - x1);
    let iy = y1 + t * (y2 - y1);

    Some((ix, iy))
}

fn in_bounds(p: (f32, f32), min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> bool {
    p.0 >= min_x && p.0 <= max_x && p.1 >= min_y && p.1 <= max_y
}

/// Check if a quadrilateral (4 points in order) is convex.
fn is_convex(corners: &[(f32, f32); 4]) -> bool {
    let mut sign = 0i32;
    for i in 0..4 {
        let (x0, y0) = corners[i];
        let (x1, y1) = corners[(i + 1) % 4];
        let (x2, y2) = corners[(i + 2) % 4];
        let cross = (x1 - x0) * (y2 - y1) - (y1 - y0) * (x2 - x1);
        let s = if cross > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return false;
        }
    }
    true
}

/// Area of a polygon using the shoelace formula.
fn polygon_area(corners: &[(f32, f32); 4]) -> f32 {
    let mut area = 0.0f32;
    for i in 0..4 {
        let (x0, y0) = corners[i];
        let (x1, y1) = corners[(i + 1) % 4];
        area += x0 * y1 - x1 * y0;
    }
    area.abs() * 0.5
}

/// Interior angle at vertex p1, with edges from p0→p1 and p1→p2.
fn interior_angle(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) -> f32 {
    let (ax, ay) = (p0.0 - p1.0, p0.1 - p1.1);
    let (bx, by) = (p2.0 - p1.0, p2.1 - p1.1);
    let dot = ax * bx + ay * by;
    let mag_a = (ax * ax + ay * ay).sqrt();
    let mag_b = (bx * bx + by * by).sqrt();
    if mag_a < 1e-6 || mag_b < 1e-6 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(-1.0, 1.0).acos()
}

/// Test if a point is inside a convex quadrilateral using cross products.
fn point_in_quad(x: f32, y: f32, corners: &[(f32, f32); 4]) -> bool {
    let mut sign = 0i32;
    for i in 0..4 {
        let (x0, y0) = corners[i];
        let (x1, y1) = corners[(i + 1) % 4];
        let cross = (x1 - x0) * (y - y0) - (y1 - y0) * (x - x0);
        let s = if cross > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convex_quad_is_convex() {
        let quad = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        assert!(is_convex(&quad));
    }

    #[test]
    fn concave_quad_is_not_convex() {
        let quad = [(0.0, 0.0), (100.0, 0.0), (50.0, 50.0), (0.0, 100.0)];
        assert!(!is_convex(&quad));
    }

    #[test]
    fn polygon_area_correct() {
        let quad = [(0.0f32, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let area = polygon_area(&quad);
        assert!(
            (area - 10000.0).abs() < 1.0,
            "100×100 square area should be 10000, got {area}"
        );
    }

    #[test]
    fn point_in_quad_works() {
        let quad = [(10.0f32, 10.0), (90.0, 10.0), (90.0, 90.0), (10.0, 90.0)];
        assert!(point_in_quad(50.0, 50.0, &quad)); // center
        assert!(!point_in_quad(0.0, 0.0, &quad)); // outside
        assert!(!point_in_quad(95.0, 50.0, &quad)); // outside right
    }

    #[test]
    fn line_intersection() {
        let h = LineSegment {
            x1: 0.0,
            y1: 50.0,
            x2: 100.0,
            y2: 50.0,
            width: 1.0,
            length: 100.0,
            angle: 0.0,
            nfa: 1.0,
        };
        let v = LineSegment {
            x1: 50.0,
            y1: 0.0,
            x2: 50.0,
            y2: 100.0,
            width: 1.0,
            length: 100.0,
            angle: core::f32::consts::FRAC_PI_2,
            nfa: 1.0,
        };
        let p = intersect_lines(&h, &v).unwrap();
        assert!(
            (p.0 - 50.0).abs() < 0.1 && (p.1 - 50.0).abs() < 0.1,
            "intersection should be (50,50), got ({:.1},{:.1})",
            p.0,
            p.1
        );
    }

    #[test]
    fn parallel_lines_no_intersection() {
        let a = LineSegment {
            x1: 0.0,
            y1: 10.0,
            x2: 100.0,
            y2: 10.0,
            width: 1.0,
            length: 100.0,
            angle: 0.0,
            nfa: 1.0,
        };
        let b = LineSegment {
            x1: 0.0,
            y1: 50.0,
            x2: 100.0,
            y2: 50.0,
            width: 1.0,
            length: 100.0,
            angle: 0.0,
            nfa: 1.0,
        };
        assert!(intersect_lines(&a, &b).is_none());
    }

    #[test]
    fn interior_angle_right_angle() {
        let a = interior_angle((0.0, 0.0), (0.0, 50.0), (50.0, 50.0));
        let degrees = a.to_degrees();
        assert!(
            (degrees - 90.0).abs() < 1.0,
            "should be ~90°, got {degrees:.1}°"
        );
    }

    #[test]
    fn score_quad_basic() {
        let (w, h) = (100usize, 100usize);
        // White rectangle on dark background
        let mut l_plane = vec![0.1f32; w * h];
        let mut grad_mag = vec![0.0f32; w * h];
        for y in 20..80 {
            for x in 20..80 {
                l_plane[y * w + x] = 0.9;
            }
        }
        // Put gradient at edges
        for y in 20..80 {
            grad_mag[y * w + 20] = 0.5;
            grad_mag[y * w + 79] = 0.5;
        }
        for x in 20..80 {
            grad_mag[20 * w + x] = 0.5;
            grad_mag[79 * w + x] = 0.5;
        }

        let corners = [(20.0f32, 20.0), (79.0, 20.0), (79.0, 79.0), (20.0, 79.0)];
        let score = score_quad(&corners, &l_plane, &grad_mag, w as u32, h as u32);
        assert!(
            score > 0.0,
            "matching quad should have positive score, got {score}"
        );
    }
}
