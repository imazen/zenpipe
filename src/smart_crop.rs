//! Content-aware smart cropping for multiple aspect ratios.
//!
//! Given focus regions (faces, objects) and/or a saliency heatmap, computes
//! optimal crop rectangles at various aspect ratios. Designed for UIs where
//! users pick from several crop candidates overlaid on the source image.
//!
//! Two modes per aspect ratio:
//! - **Minimal**: largest crop at target ratio, positioned to keep subjects visible.
//! - **Maximal**: tightest crop at target ratio, zoomed in on the subject.
//!
//! # Usage
//!
//! ```
//! use zenlayout::smart_crop::*;
//!
//! // From any detection source — or manually authored
//! let faces = vec![
//!     FocusRect { x1: 40.0, y1: 30.0, x2: 60.0, y2: 60.0, weight: 0.9 },
//! ];
//! let input = SmartCropInput { focus_regions: faces, heatmap: None };
//!
//! // Generate crop candidates for a UI picker
//! let targets = [
//!     (AspectRatio { w: 9, h: 16 }, CropMode::Minimal),
//!     (AspectRatio { w: 1, h: 1 }, CropMode::Minimal),
//!     (AspectRatio { w: 16, h: 9 }, CropMode::Minimal),
//! ];
//! let crops = input.compute_crops(1920, 1080, &targets);
//! ```
//!
//! Requires the `alloc` feature.

use alloc::vec::Vec;

use crate::Rect;

/// A weighted region of interest, in percentage coordinates (0.0–100.0).
///
/// Typically produced by face detection, object detection, or manual annotation.
/// The `weight` field controls priority when regions compete for crop position
/// (e.g., confidence from a detector, or user-assigned importance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusRect {
    /// Left edge as percentage of image width (0.0–100.0).
    pub x1: f32,
    /// Top edge as percentage of image height (0.0–100.0).
    pub y1: f32,
    /// Right edge as percentage of image width (0.0–100.0).
    pub x2: f32,
    /// Bottom edge as percentage of image height (0.0–100.0).
    pub y2: f32,
    /// Importance weight (0.0–1.0). Regions below 0.5 are ignored.
    pub weight: f32,
}

/// A 2D heatmap of per-pixel importance, row-major.
///
/// Typically a saliency map from a neural network, but could be any
/// importance signal (e.g., depth map, user-painted mask).
/// Values should be in \[0.0, 1.0\] where 1.0 = most important.
#[derive(Debug, Clone)]
pub struct HeatMap {
    /// Importance values in \[0.0, 1.0\], row-major order.
    pub data: Vec<f32>,
    /// Width of the heatmap grid.
    pub width: u32,
    /// Height of the heatmap grid.
    pub height: u32,
}

/// Target aspect ratio as integer width:height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AspectRatio {
    pub w: u32,
    pub h: u32,
}

pub const PORTRAIT_9_16: AspectRatio = AspectRatio { w: 9, h: 16 };
pub const PORTRAIT_3_4: AspectRatio = AspectRatio { w: 3, h: 4 };
pub const PORTRAIT_4_5: AspectRatio = AspectRatio { w: 4, h: 5 };
pub const SQUARE: AspectRatio = AspectRatio { w: 1, h: 1 };
pub const LANDSCAPE_16_9: AspectRatio = AspectRatio { w: 16, h: 9 };
pub const LANDSCAPE_4_3: AspectRatio = AspectRatio { w: 4, h: 3 };

/// Crop strategy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CropMode {
    /// Largest crop at target ratio. Removes minimum content.
    Minimal,
    /// Tightest crop at target ratio. Zooms in on the subject.
    Maximal,
}

/// Configuration for [`compute_crop`].
#[derive(Debug, Clone)]
pub struct CropConfig {
    /// Target aspect ratio (default: 9:16 portrait).
    pub target_aspect: AspectRatio,
    /// Crop strategy (default: Minimal).
    pub mode: CropMode,
    /// Where to place the primary focus region center vertically within the crop,
    /// as a fraction from the top (default: 0.38 — eyes land near the top third).
    pub focus_vertical_position: f32,
    /// Minimum fraction of each focus region's area that must remain inside the crop
    /// (minimal mode, default: 0.7).
    pub min_focus_visibility: f32,
    /// Padding around the subject as a fraction of subject size
    /// (maximal mode, default: 0.5).
    pub zoom_padding: f32,
}

impl Default for CropConfig {
    fn default() -> Self {
        Self {
            target_aspect: PORTRAIT_9_16,
            mode: CropMode::Minimal,
            focus_vertical_position: 0.38,
            min_focus_visibility: 0.7,
            zoom_padding: 0.5,
        }
    }
}

/// Input for smart crop computation.
///
/// Holds focus regions and an optional heatmap from any source —
/// ML detectors, manual annotation, or programmatic generation.
/// Call [`compute_crops`](SmartCropInput::compute_crops) to generate
/// crop candidates for multiple aspect ratios from a single set of inputs.
#[derive(Debug, Clone)]
pub struct SmartCropInput {
    /// Weighted regions of interest (faces, objects, etc.).
    pub focus_regions: Vec<FocusRect>,
    /// Optional importance heatmap (saliency, depth, etc.).
    pub heatmap: Option<HeatMap>,
}

impl SmartCropInput {
    /// Compute optimal crops for multiple (ratio, mode) pairs.
    ///
    /// Uses default `CropConfig` parameters (focus_vertical_position=0.38,
    /// min_focus_visibility=0.7, zoom_padding=0.5) for each entry.
    pub fn compute_crops(
        &self,
        src_w: u32,
        src_h: u32,
        targets: &[(AspectRatio, CropMode)],
    ) -> Vec<Option<Rect>> {
        targets
            .iter()
            .map(|&(ratio, mode)| {
                let config = CropConfig {
                    target_aspect: ratio,
                    mode,
                    ..CropConfig::default()
                };
                compute_crop(
                    src_w,
                    src_h,
                    &self.focus_regions,
                    self.heatmap.as_ref(),
                    &config,
                )
            })
            .collect()
    }

    /// Compute a single crop with full control over parameters.
    pub fn compute_crop(&self, src_w: u32, src_h: u32, config: &CropConfig) -> Option<Rect> {
        compute_crop(
            src_w,
            src_h,
            &self.focus_regions,
            self.heatmap.as_ref(),
            config,
        )
    }
}

/// Compute the optimal crop rectangle for the given source image.
///
/// Returns `None` if the source dimensions are degenerate (zero width or height).
pub fn compute_crop(
    src_w: u32,
    src_h: u32,
    focus_regions: &[FocusRect],
    heatmap: Option<&HeatMap>,
    config: &CropConfig,
) -> Option<Rect> {
    if src_w == 0 || src_h == 0 || config.target_aspect.w == 0 || config.target_aspect.h == 0 {
        return None;
    }

    let qualifying: Vec<&FocusRect> = focus_regions.iter().filter(|f| f.weight >= 0.5).collect();

    match config.mode {
        CropMode::Minimal => minimal_crop(src_w, src_h, &qualifying, heatmap, config),
        CropMode::Maximal => maximal_crop(src_w, src_h, &qualifying, heatmap, config),
    }
}

// ---------------------------------------------------------------------------
// Minimal mode
// ---------------------------------------------------------------------------

fn minimal_crop(
    src_w: u32,
    src_h: u32,
    regions: &[&FocusRect],
    heatmap: Option<&HeatMap>,
    config: &CropConfig,
) -> Option<Rect> {
    let (crop_w, crop_h) = largest_rect_at_ratio(src_w, src_h, config.target_aspect);
    if crop_w == 0 || crop_h == 0 {
        return None;
    }

    let sw = src_w as f64;
    let sh = src_h as f64;
    let cw = crop_w as f64;
    let ch = crop_h as f64;

    let (focus_x, focus_y, has_regions) = find_focus(regions, heatmap, sw, sh);

    // Position crop
    let cx = focus_x - cw / 2.0;

    let cy = if has_regions {
        let primary = primary_region(regions);
        let pcy = region_center_y(primary, sh);
        pcy - ch * config.focus_vertical_position as f64
    } else {
        focus_y - ch / 2.0
    };

    let mut x = clamp_f64(cx, 0.0, sw - cw);
    let mut y = clamp_f64(cy, 0.0, sh - ch);

    if has_regions {
        let primary = primary_region(regions);
        let geom = CropGeom { cw, ch, sw, sh };
        shift_for_focus_visibility(
            &mut x,
            &mut y,
            &geom,
            regions,
            primary,
            config.min_focus_visibility,
        );
    } else if let Some(hm) = heatmap {
        shift_for_heatmap_coverage(&mut x, &mut y, cw, ch, sw, sh, hm);
    }

    Some(Rect {
        x: x.round() as u32,
        y: y.round() as u32,
        width: crop_w,
        height: crop_h,
    })
}

// ---------------------------------------------------------------------------
// Maximal mode
// ---------------------------------------------------------------------------

fn maximal_crop(
    src_w: u32,
    src_h: u32,
    regions: &[&FocusRect],
    heatmap: Option<&HeatMap>,
    config: &CropConfig,
) -> Option<Rect> {
    let sw = src_w as f64;
    let sh = src_h as f64;

    let (mut sx1, mut sy1, mut sx2, mut sy2, has_regions) =
        subject_region(regions, heatmap, sw, sh, config.zoom_padding);

    expand_to_aspect(&mut sx1, &mut sy1, &mut sx2, &mut sy2, config.target_aspect);

    // Enforce minimum size (30% of source on each axis)
    let min_w = sw * 0.3;
    let min_h = sh * 0.3;
    let cur_w = sx2 - sx1;
    let cur_h = sy2 - sy1;
    if cur_w < min_w || cur_h < min_h {
        let scale = f64::max(min_w / cur_w, min_h / cur_h);
        let new_w = cur_w * scale;
        let new_h = cur_h * scale;
        let mid_x = (sx1 + sx2) / 2.0;
        let mid_y = (sy1 + sy2) / 2.0;
        sx1 = mid_x - new_w / 2.0;
        sy1 = mid_y - new_h / 2.0;
        sx2 = mid_x + new_w / 2.0;
        sy2 = mid_y + new_h / 2.0;
        expand_to_aspect(&mut sx1, &mut sy1, &mut sx2, &mut sy2, config.target_aspect);
    }

    // Headroom adjustment (focus region mode)
    if has_regions {
        let primary = primary_region(regions);
        let pcy = region_center_y(primary, sh);
        let crop_h = sy2 - sy1;
        let desired_top = pcy - crop_h * config.focus_vertical_position as f64;
        let shift = desired_top - sy1;
        sy1 += shift;
        sy2 += shift;
    }

    clamp_rect_to_bounds(&mut sx1, &mut sy1, &mut sx2, &mut sy2, sw, sh);
    enforce_aspect_after_clamp(
        &mut sx1,
        &mut sy1,
        &mut sx2,
        &mut sy2,
        sw,
        sh,
        config.target_aspect,
    );

    let crop_w = (sx2 - sx1).round() as u32;
    let crop_h = (sy2 - sy1).round() as u32;
    if crop_w == 0 || crop_h == 0 {
        return None;
    }

    Some(Rect {
        x: sx1.round() as u32,
        y: sy1.round() as u32,
        width: crop_w,
        height: crop_h,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Largest rectangle at the given aspect ratio that fits within src dimensions.
fn largest_rect_at_ratio(src_w: u32, src_h: u32, ratio: AspectRatio) -> (u32, u32) {
    let rw = ratio.w as f64;
    let rh = ratio.h as f64;
    let sw = src_w as f64;
    let sh = src_h as f64;

    let h_if_w = sw * rh / rw;
    if h_if_w <= sh {
        (src_w, h_if_w.floor() as u32)
    } else {
        let w_if_h = sh * rw / rh;
        (w_if_h.floor() as u32, src_h)
    }
}

/// Find the focus point for positioning the crop.
fn find_focus(
    regions: &[&FocusRect],
    heatmap: Option<&HeatMap>,
    sw: f64,
    sh: f64,
) -> (f64, f64, bool) {
    if !regions.is_empty() {
        let (all_x1, _, all_x2, _) = enclosing_bbox_pixels(regions, sw, sh);
        let focus_x = (all_x1 + all_x2) / 2.0;
        let primary = primary_region(regions);
        let focus_y = region_center_y(primary, sh);
        (focus_x, focus_y, true)
    } else if let Some(hm) = heatmap {
        let (cx, cy) = heatmap_center_of_mass(hm);
        (cx * sw, cy * sh, false)
    } else {
        (sw / 2.0, sh / 2.0, false)
    }
}

/// Determine subject region for maximal mode.
fn subject_region(
    regions: &[&FocusRect],
    heatmap: Option<&HeatMap>,
    sw: f64,
    sh: f64,
    zoom_padding: f32,
) -> (f64, f64, f64, f64, bool) {
    let pad = zoom_padding as f64;

    if !regions.is_empty() {
        let primary = primary_region(regions);

        // Find which regions are "close" to primary (within 2x primary width)
        let pw = (primary.x2 - primary.x1) as f64 / 100.0 * sw;
        let pcx = (primary.x1 as f64 + primary.x2 as f64) / 2.0 / 100.0 * sw;
        let pcy = (primary.y1 as f64 + primary.y2 as f64) / 2.0 / 100.0 * sh;

        let close: Vec<&&FocusRect> = regions
            .iter()
            .filter(|f| {
                let fcx = (f.x1 as f64 + f.x2 as f64) / 2.0 / 100.0 * sw;
                let fcy = (f.y1 as f64 + f.y2 as f64) / 2.0 / 100.0 * sh;
                let dist = ((fcx - pcx) * (fcx - pcx) + (fcy - pcy) * (fcy - pcy)).sqrt();
                dist <= pw * 2.0
            })
            .collect();

        let close_refs: Vec<&FocusRect> = close.iter().map(|f| **f).collect();
        let (bx1, by1, bx2, by2) = enclosing_bbox_pixels(&close_refs, sw, sh);
        let bw = bx2 - bx1;
        let bh = by2 - by1;

        (
            bx1 - bw * pad,
            by1 - bh * pad,
            bx2 + bw * pad,
            by2 + bh * pad,
            true,
        )
    } else if let Some(hm) = heatmap {
        let (hx1, hy1, hx2, hy2) = heatmap_bbox(hm, 0.5);
        let bx1 = hx1 * sw;
        let by1 = hy1 * sh;
        let bx2 = hx2 * sw;
        let by2 = hy2 * sh;
        let bw = bx2 - bx1;
        let bh = by2 - by1;
        (
            bx1 - bw * pad,
            by1 - bh * pad,
            bx2 + bw * pad,
            by2 + bh * pad,
            false,
        )
    } else {
        (sw * 0.25, sh * 0.25, sw * 0.75, sh * 0.75, false)
    }
}

/// Primary region = largest by area.
fn primary_region<'a>(regions: &[&'a FocusRect]) -> &'a FocusRect {
    regions
        .iter()
        .max_by(|a, b| {
            let area_a = (a.x2 - a.x1) * (a.y2 - a.y1);
            let area_b = (b.x2 - b.x1) * (b.y2 - b.y1);
            area_a
                .partial_cmp(&area_b)
                .unwrap_or(core::cmp::Ordering::Equal)
        })
        .unwrap()
}

/// Region center Y in pixel coordinates.
fn region_center_y(region: &FocusRect, src_h: f64) -> f64 {
    (region.y1 as f64 + region.y2 as f64) / 2.0 / 100.0 * src_h
}

/// Bounding box enclosing all regions, in pixel coordinates.
fn enclosing_bbox_pixels(regions: &[&FocusRect], sw: f64, sh: f64) -> (f64, f64, f64, f64) {
    let mut x1 = f64::MAX;
    let mut y1 = f64::MAX;
    let mut x2 = f64::MIN;
    let mut y2 = f64::MIN;
    for f in regions {
        x1 = x1.min(f.x1 as f64 / 100.0 * sw);
        y1 = y1.min(f.y1 as f64 / 100.0 * sh);
        x2 = x2.max(f.x2 as f64 / 100.0 * sw);
        y2 = y2.max(f.y2 as f64 / 100.0 * sh);
    }
    (x1, y1, x2, y2)
}

/// Square-weighted center of mass of heatmap (threshold 0.3).
/// Returns (cx, cy) normalized to [0, 1].
fn heatmap_center_of_mass(hm: &HeatMap) -> (f64, f64) {
    let mut sum_wx = 0.0_f64;
    let mut sum_wy = 0.0_f64;
    let mut sum_w = 0.0_f64;

    for row in 0..hm.height {
        for col in 0..hm.width {
            let v = hm.data[(row * hm.width + col) as usize] as f64;
            if v < 0.3 {
                continue;
            }
            let w = v * v;
            sum_wx += (col as f64 + 0.5) * w;
            sum_wy += (row as f64 + 0.5) * w;
            sum_w += w;
        }
    }

    if sum_w < 1e-10 {
        (0.5, 0.5)
    } else {
        (
            sum_wx / sum_w / hm.width as f64,
            sum_wy / sum_w / hm.height as f64,
        )
    }
}

/// Bounding box of heatmap pixels above `threshold` fraction of max.
/// Returns (x1, y1, x2, y2) normalized to [0, 1].
fn heatmap_bbox(hm: &HeatMap, threshold: f64) -> (f64, f64, f64, f64) {
    let max_val = hm.data.iter().cloned().fold(0.0_f32, f32::max) as f64;
    if max_val < 1e-10 {
        return (0.25, 0.25, 0.75, 0.75);
    }
    let thresh = max_val * threshold;

    let mut min_col = hm.width;
    let mut min_row = hm.height;
    let mut max_col = 0u32;
    let mut max_row = 0u32;

    for row in 0..hm.height {
        for col in 0..hm.width {
            if hm.data[(row * hm.width + col) as usize] as f64 >= thresh {
                min_col = min_col.min(col);
                min_row = min_row.min(row);
                max_col = max_col.max(col);
                max_row = max_row.max(row);
            }
        }
    }

    if max_col < min_col {
        return (0.25, 0.25, 0.75, 0.75);
    }

    (
        min_col as f64 / hm.width as f64,
        min_row as f64 / hm.height as f64,
        (max_col + 1) as f64 / hm.width as f64,
        (max_row + 1) as f64 / hm.height as f64,
    )
}

/// Expand a rectangle to match the target aspect ratio by growing the shorter dimension.
fn expand_to_aspect(x1: &mut f64, y1: &mut f64, x2: &mut f64, y2: &mut f64, ratio: AspectRatio) {
    let cur_w = *x2 - *x1;
    let cur_h = *y2 - *y1;
    let target_ratio = ratio.w as f64 / ratio.h as f64;
    let cur_ratio = cur_w / cur_h;

    if cur_ratio < target_ratio {
        let new_w = cur_h * target_ratio;
        let mid_x = (*x1 + *x2) / 2.0;
        *x1 = mid_x - new_w / 2.0;
        *x2 = mid_x + new_w / 2.0;
    } else {
        let new_h = cur_w / target_ratio;
        let mid_y = (*y1 + *y2) / 2.0;
        *y1 = mid_y - new_h / 2.0;
        *y2 = mid_y + new_h / 2.0;
    }
}

/// Clamp a rectangle to image bounds, preserving size.
fn clamp_rect_to_bounds(x1: &mut f64, y1: &mut f64, x2: &mut f64, y2: &mut f64, sw: f64, sh: f64) {
    let w = *x2 - *x1;
    let h = *y2 - *y1;
    if *x1 < 0.0 {
        *x1 = 0.0;
        *x2 = w.min(sw);
    }
    if *y1 < 0.0 {
        *y1 = 0.0;
        *y2 = h.min(sh);
    }
    if *x2 > sw {
        *x2 = sw;
        *x1 = (sw - w).max(0.0);
    }
    if *y2 > sh {
        *y2 = sh;
        *y1 = (sh - h).max(0.0);
    }
}

/// After clamping to image bounds, the aspect ratio may be wrong if the rect
/// was larger than the image. Shrink the excess dimension to restore the ratio.
fn enforce_aspect_after_clamp(
    x1: &mut f64,
    y1: &mut f64,
    x2: &mut f64,
    y2: &mut f64,
    sw: f64,
    sh: f64,
    ratio: AspectRatio,
) {
    let cur_w = *x2 - *x1;
    let cur_h = *y2 - *y1;
    let target_ratio = ratio.w as f64 / ratio.h as f64;
    let cur_ratio = cur_w / cur_h;
    let tolerance = 0.001;

    if (cur_ratio - target_ratio).abs() < tolerance {
        return;
    }

    if cur_ratio > target_ratio {
        let new_w = cur_h * target_ratio;
        let mid_x = (*x1 + *x2) / 2.0;
        *x1 = mid_x - new_w / 2.0;
        *x2 = mid_x + new_w / 2.0;
    } else {
        let new_h = cur_w / target_ratio;
        let mid_y = (*y1 + *y2) / 2.0;
        *y1 = mid_y - new_h / 2.0;
        *y2 = mid_y + new_h / 2.0;
    }

    if *x1 < 0.0 {
        *x2 -= *x1;
        *x1 = 0.0;
    }
    if *y1 < 0.0 {
        *y2 -= *y1;
        *y1 = 0.0;
    }
    if *x2 > sw {
        *x1 -= *x2 - sw;
        *x2 = sw;
        *x1 = x1.max(0.0);
    }
    if *y2 > sh {
        *y1 -= *y2 - sh;
        *y2 = sh;
        *y1 = y1.max(0.0);
    }
}

/// Fraction of a focus region's area that lies inside the crop rectangle.
fn region_overlap_fraction(
    region: &FocusRect,
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
    sw: f64,
    sh: f64,
) -> f64 {
    let fx1 = region.x1 as f64 / 100.0 * sw;
    let fy1 = region.y1 as f64 / 100.0 * sh;
    let fx2 = region.x2 as f64 / 100.0 * sw;
    let fy2 = region.y2 as f64 / 100.0 * sh;
    let area = (fx2 - fx1) * (fy2 - fy1);
    if area < 1e-10 {
        return 1.0;
    }

    let ox1 = fx1.max(cx);
    let oy1 = fy1.max(cy);
    let ox2 = fx2.min(cx + cw);
    let oy2 = fy2.min(cy + ch);
    let overlap = (ox2 - ox1).max(0.0) * (oy2 - oy1).max(0.0);
    overlap / area
}

struct CropGeom {
    cw: f64,
    ch: f64,
    sw: f64,
    sh: f64,
}

/// Shift crop position to improve focus region visibility.
fn shift_for_focus_visibility(
    x: &mut f64,
    y: &mut f64,
    geom: &CropGeom,
    regions: &[&FocusRect],
    primary: &FocusRect,
    min_visibility: f32,
) {
    let min_vis = min_visibility as f64;
    let CropGeom { cw, ch, sw, sh } = *geom;

    for region in regions {
        let frac = region_overlap_fraction(region, *x, *y, cw, ch, sw, sh);
        if frac >= min_vis {
            continue;
        }

        let fx1 = region.x1 as f64 / 100.0 * sw;
        let fy1 = region.y1 as f64 / 100.0 * sh;
        let fx2 = region.x2 as f64 / 100.0 * sw;
        let fy2 = region.y2 as f64 / 100.0 * sh;

        if fx1 < *x {
            *x = clamp_f64(fx1, 0.0, sw - cw);
        }
        if fx2 > *x + cw {
            *x = clamp_f64(fx2 - cw, 0.0, sw - cw);
        }
        if fy1 < *y {
            *y = clamp_f64(fy1, 0.0, sh - ch);
        }
        if fy2 > *y + ch {
            *y = clamp_f64(fy2 - ch, 0.0, sh - ch);
        }
    }

    let primary_frac = region_overlap_fraction(primary, *x, *y, cw, ch, sw, sh);
    if primary_frac < min_vis {
        let fx1 = primary.x1 as f64 / 100.0 * sw;
        let fy1 = primary.y1 as f64 / 100.0 * sh;
        let fx2 = primary.x2 as f64 / 100.0 * sw;
        let fy2 = primary.y2 as f64 / 100.0 * sh;
        let fcx = (fx1 + fx2) / 2.0;
        let fcy = (fy1 + fy2) / 2.0;
        *x = clamp_f64(fcx - cw / 2.0, 0.0, sw - cw);
        *y = clamp_f64(fcy - ch / 2.0, 0.0, sh - ch);
    }
}

/// Shift crop to maximize coverage of the hot region in the heatmap.
fn shift_for_heatmap_coverage(
    x: &mut f64,
    y: &mut f64,
    cw: f64,
    ch: f64,
    sw: f64,
    sh: f64,
    hm: &HeatMap,
) {
    let (bx1, by1, bx2, by2) = heatmap_bbox(hm, 0.3);
    let hm_x1 = bx1 * sw;
    let hm_y1 = by1 * sh;
    let hm_x2 = bx2 * sw;
    let hm_y2 = by2 * sh;

    if hm_y1 < *y {
        // Hot region extends above the crop (or is taller than the crop) — prioritize top
        *y = clamp_f64(hm_y1, 0.0, sh - ch);
    } else if hm_y2 > *y + ch {
        *y = clamp_f64(hm_y2 - ch, 0.0, sh - ch);
    }

    if hm_x1 < *x && hm_x2 > *x + cw {
        let hm_cx = (hm_x1 + hm_x2) / 2.0;
        *x = clamp_f64(hm_cx - cw / 2.0, 0.0, sw - cw);
    } else if hm_x1 < *x {
        *x = clamp_f64(hm_x1, 0.0, sw - cw);
    } else if hm_x2 > *x + cw {
        *x = clamp_f64(hm_x2 - cw, 0.0, sw - cw);
    }
}

fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(x1: f32, y1: f32, x2: f32, y2: f32, weight: f32) -> FocusRect {
        FocusRect {
            x1,
            y1,
            x2,
            y2,
            weight,
        }
    }

    fn make_heatmap(width: u32, height: u32, hot_spots: &[(u32, u32, f32)]) -> HeatMap {
        let mut data = vec![0.0f32; (width * height) as usize];
        for &(col, row, val) in hot_spots {
            if col < width && row < height {
                data[(row * width + col) as usize] = val;
            }
        }
        HeatMap {
            data,
            width,
            height,
        }
    }

    fn make_heatmap_rect(
        width: u32,
        height: u32,
        rx1: u32,
        ry1: u32,
        rx2: u32,
        ry2: u32,
        val: f32,
    ) -> HeatMap {
        let mut data = vec![0.0f32; (width * height) as usize];
        for row in ry1..ry2.min(height) {
            for col in rx1..rx2.min(width) {
                data[(row * width + col) as usize] = val;
            }
        }
        HeatMap {
            data,
            width,
            height,
        }
    }

    fn assert_crop_inside(crop: &Rect, src_w: u32, src_h: u32) {
        assert!(
            crop.x + crop.width <= src_w,
            "crop right edge {} exceeds src width {}",
            crop.x + crop.width,
            src_w
        );
        assert!(
            crop.y + crop.height <= src_h,
            "crop bottom edge {} exceeds src height {}",
            crop.y + crop.height,
            src_h
        );
    }

    fn assert_approx_aspect(crop: &Rect, ratio: AspectRatio, tolerance: f64) {
        let actual = crop.width as f64 / crop.height as f64;
        let expected = ratio.w as f64 / ratio.h as f64;
        let diff = (actual - expected).abs();
        assert!(
            diff < tolerance,
            "aspect {actual:.4} not within {tolerance} of expected {expected:.4} (crop {}x{})",
            crop.width,
            crop.height
        );
    }

    #[test]
    fn minimal_landscape_centered_face() {
        let regions = [focus(40.0, 30.0, 60.0, 60.0, 0.9)];
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        assert_eq!(crop.height, 1080);
        let face_cy = 0.45 * 1080.0;
        let face_in_crop = face_cy - crop.y as f64;
        let frac = face_in_crop / crop.height as f64;
        assert!(
            frac > 0.25 && frac < 0.55,
            "face at {frac:.2} from top, expected ~0.38"
        );
    }

    #[test]
    fn minimal_face_far_right() {
        let regions = [focus(85.0, 30.0, 95.0, 60.0, 0.9)];
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        assert!(
            crop.x > 600,
            "crop should be shifted right, got x={}",
            crop.x
        );
    }

    #[test]
    fn minimal_three_faces_spread() {
        let regions = [
            focus(5.0, 30.0, 15.0, 60.0, 0.8),
            focus(40.0, 20.0, 60.0, 60.0, 0.95),
            focus(85.0, 30.0, 95.0, 60.0, 0.7),
        ];
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        let vis = region_overlap_fraction(
            &regions[1],
            crop.x as f64,
            crop.y as f64,
            crop.width as f64,
            crop.height as f64,
            1920.0,
            1080.0,
        );
        assert!(vis > 0.7, "primary face visibility {vis:.2} < 0.7");
    }

    #[test]
    fn minimal_heatmap_upper_left() {
        let hm = make_heatmap(
            128,
            128,
            &[
                (10, 10, 1.0),
                (11, 10, 0.9),
                (10, 11, 0.9),
                (12, 10, 0.8),
                (10, 12, 0.8),
            ],
        );
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &[], Some(&hm), &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        assert!(
            crop.x < 500,
            "crop should be left-shifted, got x={}",
            crop.x
        );
    }

    #[test]
    fn minimal_no_focus_no_heatmap() {
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &[], None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        let center_x = crop.x as f64 + crop.width as f64 / 2.0;
        let diff = (center_x - 960.0).abs();
        assert!(diff < 2.0, "expected centered, center_x={center_x}");
    }

    #[test]
    fn minimal_portrait_already_9_16() {
        let config = CropConfig::default();
        let crop = compute_crop(1080, 1920, &[], None, &config).unwrap();

        assert_crop_inside(&crop, 1080, 1920);
        assert_eq!(crop.x, 0);
        assert_eq!(crop.y, 0);
        assert_eq!(crop.width, 1080);
        assert_eq!(crop.height, 1920);
    }

    #[test]
    fn minimal_square_with_face() {
        let regions = [focus(40.0, 20.0, 60.0, 50.0, 0.9)];
        let config = CropConfig::default();
        let crop = compute_crop(1000, 1000, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1000, 1000);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.01);
        assert_eq!(crop.height, 1000);
        assert!(
            (crop.width as i32 - 562).abs() <= 1,
            "expected ~562, got {}",
            crop.width
        );
    }

    #[test]
    fn maximal_landscape_centered_face() {
        let regions = [focus(35.0, 20.0, 65.0, 70.0, 0.95)];
        let config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.02);
        assert!(
            crop.width < 1920 && crop.height <= 1080,
            "maximal should zoom in: {}x{}",
            crop.width,
            crop.height
        );
    }

    #[test]
    fn maximal_small_face_corner() {
        let regions = [focus(85.0, 75.0, 95.0, 90.0, 0.85)];
        let config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.02);
    }

    #[test]
    fn maximal_heatmap_center() {
        let hm = make_heatmap_rect(128, 128, 50, 50, 78, 78, 1.0);
        let config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = compute_crop(1920, 1080, &[], Some(&hm), &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.02);
        assert!(crop.width < 1920, "should zoom in");
    }

    #[test]
    fn maximal_faces_close() {
        let regions = [
            focus(40.0, 30.0, 55.0, 60.0, 0.95),
            focus(55.0, 32.0, 68.0, 58.0, 0.90),
        ];
        let config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.02);
        for f in &regions {
            let vis = region_overlap_fraction(
                f,
                crop.x as f64,
                crop.y as f64,
                crop.width as f64,
                crop.height as f64,
                1920.0,
                1080.0,
            );
            assert!(vis > 0.5, "face visibility {vis:.2} too low");
        }
    }

    #[test]
    fn maximal_faces_far_apart() {
        let regions = [
            focus(10.0, 30.0, 25.0, 60.0, 0.95),
            focus(80.0, 30.0, 90.0, 55.0, 0.80),
        ];
        let config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();

        assert_crop_inside(&crop, 1920, 1080);
        assert_approx_aspect(&crop, PORTRAIT_9_16, 0.02);
        let vis = region_overlap_fraction(
            &regions[0],
            crop.x as f64,
            crop.y as f64,
            crop.width as f64,
            crop.height as f64,
            1920.0,
            1080.0,
        );
        assert!(vis > 0.7, "primary face visibility {vis:.2} too low");
    }

    #[test]
    fn source_matches_aspect() {
        let regions = [focus(40.0, 30.0, 60.0, 60.0, 0.9)];
        let min_config = CropConfig::default();
        let min_crop = compute_crop(900, 1600, &regions, None, &min_config).unwrap();
        assert_eq!(min_crop.width, 900);
        assert_eq!(min_crop.height, 1600);

        let max_config = CropConfig {
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let max_crop = compute_crop(900, 1600, &regions, None, &max_config).unwrap();
        assert_crop_inside(&max_crop, 900, 1600);
        assert_approx_aspect(&max_crop, PORTRAIT_9_16, 0.02);
        assert!(
            max_crop.width < 900 || max_crop.height < 1600,
            "maximal should zoom in"
        );
    }

    #[test]
    fn degenerate_zero_width() {
        let config = CropConfig::default();
        assert!(compute_crop(0, 1080, &[], None, &config).is_none());
    }

    #[test]
    fn degenerate_zero_height() {
        let config = CropConfig::default();
        assert!(compute_crop(1920, 0, &[], None, &config).is_none());
    }

    #[test]
    fn low_weight_regions_ignored() {
        let regions = [focus(40.0, 30.0, 60.0, 60.0, 0.3)];
        let config = CropConfig::default();
        let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();
        let center_x = crop.x as f64 + crop.width as f64 / 2.0;
        let diff = (center_x - 960.0).abs();
        assert!(
            diff < 2.0,
            "expected centered (no qualifying regions), got center_x={center_x}"
        );
    }

    #[test]
    fn all_aspect_ratios_produce_valid_crops() {
        let regions = [focus(40.0, 30.0, 60.0, 60.0, 0.9)];
        for &ratio in &[
            PORTRAIT_9_16,
            PORTRAIT_3_4,
            PORTRAIT_4_5,
            SQUARE,
            LANDSCAPE_16_9,
            LANDSCAPE_4_3,
        ] {
            for &mode in &[CropMode::Minimal, CropMode::Maximal] {
                let config = CropConfig {
                    target_aspect: ratio,
                    mode,
                    ..CropConfig::default()
                };
                let crop = compute_crop(1920, 1080, &regions, None, &config).unwrap();
                assert_crop_inside(&crop, 1920, 1080);
                assert_approx_aspect(&crop, ratio, 0.03);
            }
        }
    }

    #[test]
    fn batch_matches_individual() {
        let regions = [focus(40.0, 30.0, 60.0, 60.0, 0.9)];
        let input = SmartCropInput {
            focus_regions: regions.to_vec(),
            heatmap: None,
        };

        let targets = [
            (PORTRAIT_9_16, CropMode::Minimal),
            (SQUARE, CropMode::Maximal),
            (LANDSCAPE_16_9, CropMode::Minimal),
        ];

        let batch = input.compute_crops(1920, 1080, &targets);

        for (i, &(ratio, mode)) in targets.iter().enumerate() {
            let config = CropConfig {
                target_aspect: ratio,
                mode,
                ..CropConfig::default()
            };
            let single = input.compute_crop(1920, 1080, &config);
            assert_eq!(
                batch[i], single,
                "batch[{i}] should match individual compute_crop"
            );
        }
    }
}
