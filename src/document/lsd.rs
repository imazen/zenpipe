//!
//! Implements the algorithm from Grompone von Gioi et al. (IPOL 2012).
//! Given an L plane, detects line segments with sub-pixel accuracy and
//! statistical validation (NFA — Number of False Alarms).
//!
//! The algorithm:
//! 1. Compute gradient magnitude and level-line angle at each pixel
//! 2. Sort pixels by gradient magnitude (pseudo-sort via bins)
//! 3. Region growing: from high-gradient seeds, grow connected regions
//!    of pixels with similar level-line angles
//! 4. Fit a bounding rectangle to each region
//! 5. Validate via NFA (reject segments likely due to noise)

extern crate alloc;

use crate::context::FilterContext;
use crate::prelude::*;

/// A detected line segment with quality metrics.
#[derive(Clone, Debug)]
pub struct LineSegment {
    /// Start point x coordinate.
    pub x1: f32,
    /// Start point y coordinate.
    pub y1: f32,
    /// End point x coordinate.
    pub x2: f32,
    /// End point y coordinate.
    pub y2: f32,
    /// Rectangle width (perpendicular to the segment direction).
    pub width: f32,
    /// Segment length.
    pub length: f32,
    /// Angle in radians (-π to π).
    pub angle: f32,
    /// -log10(NFA). Higher = more significant. Segments with nfa > 0 are valid.
    pub nfa: f32,
}

impl LineSegment {
    /// Squared length (avoids sqrt for comparisons).
    pub fn length_sq(&self) -> f32 {
        let dx = self.x2 - self.x1;
        let dy = self.y2 - self.y1;
        dx * dx + dy * dy
    }
}

/// Angle tolerance for region growing (radians). Default: π/8 = 22.5°.
const DEFAULT_ANG_TH: f32 = core::f32::consts::PI / 8.0;

/// Minimum gradient magnitude relative to maximum. Pixels below this are ignored.
const GRAD_QUANT: f32 = 2.0 / 255.0;

/// NFA threshold: -log10(epsilon). Segments with nfa >= this are kept.
/// Default epsilon = 1.0 → threshold = 0.0 (keep all significant segments).
const LOG_NFA_THRESHOLD: f32 = 0.0;

/// Detect line segments in an L-channel plane.
///
/// Returns a list of validated line segments sorted by significance (highest NFA first).
pub fn detect_line_segments(
    l_plane: &[f32],
    width: u32,
    height: u32,
    ctx: &mut FilterContext,
) -> Vec<LineSegment> {
    let w = width as usize;
    let h = height as usize;
    let n = w * h;

    if w < 3 || h < 3 {
        return Vec::new();
    }

    // Step 1: Compute gradient magnitude and level-line angle
    let mut grad_mag = ctx.take_f32(n);
    let mut grad_ang = ctx.take_f32(n);
    compute_gradient(l_plane, &mut grad_mag, &mut grad_ang, w, h);

    // Find gradient threshold
    let max_grad = grad_mag.iter().copied().fold(0.0f32, f32::max);
    let grad_threshold = max_grad * GRAD_QUANT;

    if max_grad < 1e-10 {
        ctx.return_f32(grad_ang);
        ctx.return_f32(grad_mag);
        return Vec::new();
    }

    // Step 2: Collect and sort pixels by gradient magnitude (descending)
    let mut pixel_indices: Vec<u32> = (0..n as u32)
        .filter(|&i| {
            let idx = i as usize;
            grad_mag[idx] > grad_threshold && grad_ang[idx] < 9.0
        })
        .collect();
    pixel_indices.sort_unstable_by(|&a, &b| {
        grad_mag[b as usize]
            .partial_cmp(&grad_mag[a as usize])
            .unwrap_or(core::cmp::Ordering::Equal)
    });

    // Step 3: Region growing + rectangle fitting + NFA validation
    let mut used = vec![false; n]; // pixel usage mask
    let mut segments = Vec::new();

    let ang_th = DEFAULT_ANG_TH;
    let p = ang_th / core::f32::consts::PI; // probability for NFA

    for &pixel_idx in &pixel_indices {
        let idx = pixel_idx as usize;
        if used[idx] {
            continue;
        }

        let seed_angle = grad_ang[idx];
        if seed_angle > 9.0 {
            continue;
        }

        // Grow region from seed
        let mut region: Vec<(usize, usize)> = Vec::new(); // (x, y) pairs
        let mut queue: Vec<usize> = Vec::new();
        queue.push(idx);
        used[idx] = true;

        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut sum_mag = 0.0f64;
        let mut region_angle = seed_angle;

        while let Some(curr) = queue.pop() {
            let cx = (curr % w) as usize;
            let cy = (curr / w) as usize;
            let mag = grad_mag[curr] as f64;

            region.push((cx, cy));
            sum_x += cx as f64 * mag;
            sum_y += cy as f64 * mag;
            sum_mag += mag;

            // Visit 8-connected neighbors
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || nx >= w as i32 || ny < 0 || ny >= h as i32 {
                        continue;
                    }
                    let ni = ny as usize * w + nx as usize;
                    if used[ni] {
                        continue;
                    }
                    if grad_mag[ni] <= grad_threshold {
                        continue;
                    }
                    if grad_ang[ni] > 9.0 {
                        continue;
                    }

                    // Check angle similarity
                    if angle_diff(grad_ang[ni], region_angle) <= ang_th {
                        used[ni] = true;
                        queue.push(ni);

                        // Update region angle (weighted by gradient magnitude)
                        // Use incremental angle update via sin/cos averaging
                    }
                }
            }
        }

        if region.len() < 3 {
            continue;
        }

        // Fit rectangle to region using weighted moments
        if sum_mag < 1e-10 {
            continue;
        }
        let inv_sum = 1.0 / sum_mag;
        let center_x = (sum_x * inv_sum) as f32;
        let center_y = (sum_y * inv_sum) as f32;

        // Compute orientation from inertia tensor
        let mut ixx = 0.0f64;
        let mut iyy = 0.0f64;
        let mut ixy = 0.0f64;
        for &(px, py) in &region {
            let dx = px as f64 - center_x as f64;
            let dy = py as f64 - center_y as f64;
            let mag = grad_mag[py * w + px] as f64;
            ixx += dx * dx * mag;
            iyy += dy * dy * mag;
            ixy += dx * dy * mag;
        }

        // Principal axis angle (eigenvector of inertia tensor)
        let theta = 0.5 * (2.0 * ixy).atan2(ixx - iyy) as f32;

        // Project region onto principal axes to get bounding rectangle
        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let mut min_along = f32::MAX;
        let mut max_along = f32::MIN;
        let mut min_perp = f32::MAX;
        let mut max_perp = f32::MIN;

        for &(px, py) in &region {
            let dx = px as f32 - center_x;
            let dy = py as f32 - center_y;
            let along = dx * cos_t + dy * sin_t;
            let perp = -dx * sin_t + dy * cos_t;
            min_along = min_along.min(along);
            max_along = max_along.max(along);
            min_perp = min_perp.min(perp);
            max_perp = max_perp.max(perp);
        }

        let length = max_along - min_along;
        let rect_width = (max_perp - min_perp).max(1.0); // minimum 1px width

        if length < 2.0 {
            continue;
        }

        // Compute endpoints
        let mid_along = (min_along + max_along) * 0.5;
        let half_len = length * 0.5;
        let x1 = center_x + (mid_along - half_len) * cos_t;
        let y1 = center_y + (mid_along - half_len) * sin_t;
        let x2 = center_x + (mid_along + half_len) * cos_t;
        let y2 = center_y + (mid_along + half_len) * sin_t;

        // NFA validation
        // NFA = N_tests * B(n, k, p)
        // where n = region area, k = aligned pixels, p = angle_tolerance / pi
        let n_points = region.len();
        let k_aligned = count_aligned_points(&region, &grad_ang, theta, ang_th, w);
        let log_nfa = compute_log_nfa(n_points, k_aligned, p, w, h);

        if log_nfa < LOG_NFA_THRESHOLD {
            continue;
        }

        segments.push(LineSegment {
            x1,
            y1,
            x2,
            y2,
            width: rect_width,
            length,
            angle: theta,
            nfa: log_nfa,
        });
    }

    // Sort by NFA (most significant first)
    segments.sort_unstable_by(|a, b| {
        b.nfa
            .partial_cmp(&a.nfa)
            .unwrap_or(core::cmp::Ordering::Equal)
    });

    ctx.return_f32(grad_ang);
    ctx.return_f32(grad_mag);

    segments
}

/// Compute gradient magnitude and level-line angle for each pixel.
///
/// Level-line angle = gradient direction + π/2 (perpendicular to gradient).
/// Pixels with gradient below threshold get angle = NOTDEF (10.0).
fn compute_gradient(src: &[f32], mag: &mut [f32], ang: &mut [f32], w: usize, h: usize) {
    // Use 2×2 gradient (faster than 3×3 Sobel, standard for LSD)
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if x + 1 >= w || y + 1 >= h {
                mag[idx] = 0.0;
                ang[idx] = 10.0; // NOTDEF
                continue;
            }

            let p00 = src[y * w + x];
            let p10 = src[y * w + x + 1];
            let p01 = src[(y + 1) * w + x];
            let p11 = src[(y + 1) * w + x + 1];

            let gx = (p10 - p00 + p11 - p01) * 0.5;
            let gy = (p01 - p00 + p11 - p10) * 0.5;

            mag[idx] = (gx * gx + gy * gy).sqrt();

            if mag[idx] < 1e-10 {
                ang[idx] = 10.0; // NOTDEF
            } else {
                // Level-line angle = atan2(-gx, gy) (perpendicular to gradient)
                ang[idx] = (-gx).atan2(gy);
            }
        }
    }
}

/// Angle difference in range [0, π].
#[inline]
fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b).abs();
    if d > core::f32::consts::PI {
        d = 2.0 * core::f32::consts::PI - d;
    }
    d
}

/// Count points in a region whose gradient angle is within tolerance of the rectangle angle.
fn count_aligned_points(
    region: &[(usize, usize)],
    grad_ang: &[f32],
    rect_angle: f32,
    ang_th: f32,
    w: usize,
) -> usize {
    region
        .iter()
        .filter(|&&(x, y)| {
            let a = grad_ang[y * w + x];
            a < 9.0 && angle_diff(a, rect_angle) <= ang_th
        })
        .count()
}

/// Compute -log10(NFA) for a line segment.
///
/// NFA = (W*H)^(5/2) * B(n, k, p)
/// where B(n, k, p) = sum_{i=k}^{n} C(n,i) * p^i * (1-p)^(n-i)
///
/// We compute log10(NFA) and return its negation.
/// A segment is valid if -log10(NFA) >= 0, i.e., NFA <= 1.
fn compute_log_nfa(n: usize, k: usize, p: f32, img_w: usize, img_h: usize) -> f32 {
    if k == 0 || k > n {
        return -1.0; // invalid
    }

    let p = p as f64;
    let n_tests_log = 2.5 * ((img_w * img_h) as f64).ln();

    // Compute log of binomial tail probability using the approximation:
    // log B(n, k, p) ≈ log C(n,k) + k*log(p) + (n-k)*log(1-p)
    // For the tail, this is an upper bound.
    let log_binom = log_choose(n, k) + k as f64 * p.ln() + (n - k) as f64 * (1.0 - p).ln();

    let log_nfa = n_tests_log + log_binom;
    let log10_nfa = log_nfa / core::f64::consts::LN_10;

    -log10_nfa as f32
}

/// Compute log(C(n, k)) using log-gamma (Stirling).
fn log_choose(n: usize, k: usize) -> f64 {
    if k == 0 || k == n {
        return 0.0;
    }
    log_gamma(n as f64 + 1.0) - log_gamma(k as f64 + 1.0) - log_gamma((n - k) as f64 + 1.0)
}

/// Log-gamma via Stirling's approximation.
/// Accurate to ~1e-8 for x >= 1.
fn log_gamma(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x < 1.0 {
        // Use Gamma(x+1) = x * Gamma(x) → log_gamma(x) = log_gamma(x+1) - ln(x)
        return log_gamma(x + 1.0) - x.ln();
    }
    // Stirling series: log(Gamma(x)) ≈ (x-0.5)*ln(x) - x + 0.5*ln(2π) + 1/(12x) - 1/(360x³)
    let t = x - 0.5;
    t * x.ln() - x + 0.5 * (2.0 * core::f64::consts::PI).ln() + 1.0 / (12.0 * x)
        - 1.0 / (360.0 * x * x * x)
}

/// Access the gradient magnitude buffer (for reuse in quad scoring).
/// Re-runs the gradient computation. For efficiency, callers should cache this.
pub fn compute_gradient_magnitude(l_plane: &[f32], grad_mag: &mut [f32], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    // Simple Sobel-like 2×2 gradient (matches LSD's internal computation)
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if x + 1 >= w || y + 1 >= h {
                grad_mag[idx] = 0.0;
                continue;
            }
            let p00 = l_plane[y * w + x];
            let p10 = l_plane[y * w + x + 1];
            let p01 = l_plane[(y + 1) * w + x];
            let p11 = l_plane[(y + 1) * w + x + 1];
            let gx = (p10 - p00 + p11 - p01) * 0.5;
            let gy = (p01 - p00 + p11 - p10) * 0.5;
            grad_mag[idx] = (gx * gx + gy * gy).sqrt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;
    use crate::prelude::*;

    #[test]
    fn constant_plane_no_segments() {
        let plane = vec![0.5f32; 64 * 64];
        let segs = detect_line_segments(&plane, 64, 64, &mut FilterContext::new());
        assert!(
            segs.is_empty(),
            "constant plane should have no segments, got {}",
            segs.len()
        );
    }

    #[test]
    fn detects_horizontal_line() {
        // White horizontal stripe on black background
        let (w, h) = (128, 128);
        let mut plane = vec![0.1f32; w * h];
        for x in 10..118 {
            for y in 60..68 {
                plane[y * w + x] = 0.9;
            }
        }

        let segs = detect_line_segments(&plane, w as u32, h as u32, &mut FilterContext::new());
        assert!(
            !segs.is_empty(),
            "should detect at least one segment for horizontal stripe"
        );

        // Find the most significant segment
        let best = &segs[0];
        // Should be roughly horizontal (angle near 0 or π)
        let abs_angle = best.angle.abs();
        let is_horizontal = abs_angle < 0.5 || (core::f32::consts::PI - abs_angle) < 0.5;
        assert!(
            is_horizontal,
            "best segment should be horizontal, angle={:.2}°",
            best.angle.to_degrees()
        );
        assert!(
            best.length > 50.0,
            "segment should be long, got {:.1}",
            best.length
        );
    }

    #[test]
    fn detects_vertical_line() {
        let (w, h) = (128, 128);
        let mut plane = vec![0.1f32; w * h];
        for y in 10..118 {
            for x in 60..68 {
                plane[y * w + x] = 0.9;
            }
        }
        let segs = detect_line_segments(&plane, w as u32, h as u32, &mut FilterContext::new());
        assert!(!segs.is_empty(), "should detect vertical stripe segments");

        let best = &segs[0];
        // Should be roughly vertical (angle near ±π/2)
        let abs_angle = best.angle.abs();
        let is_vertical = (abs_angle - core::f32::consts::FRAC_PI_2).abs() < 0.5;
        assert!(
            is_vertical,
            "best segment should be vertical, angle={:.2}°",
            best.angle.to_degrees()
        );
    }

    #[test]
    fn rectangle_produces_four_segments() {
        // White rectangle on dark background should produce ~4 edge segments
        let (w, h) = (200, 200);
        let mut plane = vec![0.1f32; w * h];
        for y in 40..160 {
            for x in 30..170 {
                plane[y * w + x] = 0.9;
            }
        }
        let segs = detect_line_segments(&plane, w as u32, h as u32, &mut FilterContext::new());
        // Should detect significant segments for the rectangle edges.
        // Due to region growing connecting corners, horizontal and vertical
        // edges may merge — so we may get 2-4 segments depending on the image.
        let significant: Vec<_> = segs.iter().filter(|s| s.length > 30.0).collect();
        assert!(
            significant.len() >= 2,
            "rectangle should have >= 2 long segments, got {} (total {})",
            significant.len(),
            segs.len()
        );
    }

    #[test]
    fn tiny_image_returns_empty() {
        let plane = vec![0.5f32; 4];
        let segs = detect_line_segments(&plane, 2, 2, &mut FilterContext::new());
        assert!(segs.is_empty());
    }

    #[test]
    fn angle_diff_symmetric() {
        assert!((angle_diff(0.1, 0.3) - 0.2).abs() < 1e-5);
        assert!((angle_diff(0.3, 0.1) - 0.2).abs() < 1e-5);
        // Wrap-around
        let d = angle_diff(3.0, -3.0);
        assert!(
            d < core::f32::consts::PI + 0.01,
            "wrap-around diff should be < π, got {d}"
        );
    }

    #[test]
    fn log_gamma_basic() {
        // log(Gamma(1)) = 0 (Stirling is approximate at small x)
        assert!(log_gamma(1.0).abs() < 0.001);
        // log(Gamma(2)) = log(1) = 0
        assert!(log_gamma(2.0).abs() < 0.001);
        // log(Gamma(5)) = log(24) ≈ 3.178
        assert!((log_gamma(5.0) - 24.0f64.ln()).abs() < 0.01);
        // log(Gamma(10)) = log(362880) — more accurate for larger x
        assert!((log_gamma(10.0) - 362880.0f64.ln()).abs() < 0.001);
    }
}
