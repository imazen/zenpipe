//! Watermark layout: gravity, margins, constraint modes.
//!
//! Resolves watermark placement from abstract constraints (gravity, margins,
//! fit mode) to absolute pixel coordinates suitable for [`NodeOp::Overlay`]
//! or [`NodeOp::Materialize`].
//!
//! This module contains **no pixel operations** — it's pure geometry.
//! Compositing is done by the pipeline (via Overlay or Materialize nodes).
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::watermark::{WatermarkLayout, FitBox, FitMode, Gravity};
//!
//! let layout = WatermarkLayout {
//!     wm_width: 200,
//!     wm_height: 100,
//!     fit_box: FitBox::Margins { left: 10, top: 10, right: 10, bottom: 10 },
//!     fit_mode: FitMode::Within,
//!     gravity: Gravity::Percentage(95.0, 95.0), // bottom-right
//!     min_canvas_width: Some(100),
//!     min_canvas_height: Some(100),
//! };
//!
//! let placement = layout.resolve(1920, 1080);
//! // placement.x, placement.y, placement.width, placement.height
//! ```

// ─── Constraint types ───

/// How to constrain the watermark within its bounding box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FitMode {
    /// Scale to exact box size, ignoring aspect ratio.
    Distort,
    /// Scale to fit within box, upscaling if needed. Preserves aspect ratio.
    Fit,
    /// Scale to fit within box, no upscaling. Preserves aspect ratio.
    #[default]
    Within,
    /// Scale to fill box (may exceed one dimension), then clip to box.
    FitCrop,
    /// Like FitCrop but no upscaling.
    WithinCrop,
}

/// How to compute the bounding box from canvas dimensions.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FitBox {
    /// Full canvas (no margins).
    #[default]
    FullCanvas,
    /// Pixel margins from canvas edges.
    Margins {
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    },
    /// Percentage-based region of the canvas.
    /// Values are 0-100 percentages: (x1%, y1%, x2%, y2%).
    Percentage { x1: f32, y1: f32, x2: f32, y2: f32 },
}

/// Gravity: where to anchor the watermark within its bounding box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Gravity {
    /// Center of the bounding box.
    #[default]
    Center,
    /// Percentage-based positioning: (x%, y%) where 0=left/top, 100=right/bottom.
    Percentage(f32, f32),
}

// ─── Layout definition ───

/// Watermark layout constraints.
///
/// Describes *where* and *how big* a watermark should be placed on a canvas,
/// without holding any pixel data.
#[derive(Clone, Debug)]
pub struct WatermarkLayout {
    /// Original watermark image width (before scaling).
    pub wm_width: u32,
    /// Original watermark image height (before scaling).
    pub wm_height: u32,
    /// How to compute the bounding box on the canvas.
    pub fit_box: FitBox,
    /// How to scale the watermark within the bounding box.
    pub fit_mode: FitMode,
    /// Where to position the watermark within the bounding box.
    pub gravity: Gravity,
    /// Skip watermark if canvas is narrower than this.
    pub min_canvas_width: Option<u32>,
    /// Skip watermark if canvas is shorter than this.
    pub min_canvas_height: Option<u32>,
}

/// Resolved watermark placement — absolute pixel coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatermarkPlacement {
    /// X position on the canvas (top-left of watermark).
    pub x: i32,
    /// Y position on the canvas (top-left of watermark).
    pub y: i32,
    /// Scaled watermark width.
    pub width: u32,
    /// Scaled watermark height.
    pub height: u32,
}

impl WatermarkLayout {
    /// Resolve the layout to absolute pixel placement on a canvas.
    ///
    /// Returns `None` if:
    /// - Canvas is smaller than `min_canvas_width`/`min_canvas_height`
    /// - Bounding box is invalid (margins exceed canvas)
    /// - Watermark or box dimensions are zero
    pub fn resolve(&self, canvas_w: u32, canvas_h: u32) -> Option<WatermarkPlacement> {
        // Check minimum canvas size.
        if self.min_canvas_width.is_some_and(|min| canvas_w < min) {
            return None;
        }
        if self.min_canvas_height.is_some_and(|min| canvas_h < min) {
            return None;
        }

        // Compute bounding box.
        let (box_x1, box_y1, box_x2, box_y2) =
            compute_bounding_box(canvas_w, canvas_h, &self.fit_box)?;

        let box_w = (box_x2 - box_x1) as u32;
        let box_h = (box_y2 - box_y1) as u32;

        // Compute target size.
        let (target_w, target_h) =
            compute_watermark_size(self.wm_width, self.wm_height, box_w, box_h, self.fit_mode);

        if target_w == 0 || target_h == 0 {
            return None;
        }

        // Compute position.
        let (x, y) = compute_gravity_position(
            box_x1,
            box_y1,
            box_x2,
            box_y2,
            target_w as i32,
            target_h as i32,
            self.gravity,
        );

        Some(WatermarkPlacement {
            x,
            y,
            width: target_w,
            height: target_h,
        })
    }
}

// ─── Bounding box computation ───

fn compute_bounding_box(w: u32, h: u32, fit_box: &FitBox) -> Option<(i32, i32, i32, i32)> {
    match fit_box {
        FitBox::FullCanvas => Some((0, 0, w as i32, h as i32)),
        FitBox::Margins {
            left,
            top,
            right,
            bottom,
        } => {
            if left + right < w && top + bottom < h {
                Some((
                    *left as i32,
                    *top as i32,
                    w as i32 - *right as i32,
                    h as i32 - *bottom as i32,
                ))
            } else {
                None
            }
        }
        FitBox::Percentage { x1, y1, x2, y2 } => {
            let to_px = |pct: f32, dim: u32| -> i32 {
                (pct.clamp(0.0, 100.0) / 100.0 * dim as f32).round() as i32
            };
            let px1 = to_px(*x1, w);
            let py1 = to_px(*y1, h);
            let px2 = to_px(*x2, w);
            let py2 = to_px(*y2, h);
            if px1 < px2 && py1 < py2 {
                Some((px1, py1, px2, py2))
            } else {
                None
            }
        }
    }
}

// ─── Constraint mode sizing ───

fn compute_watermark_size(
    wm_w: u32,
    wm_h: u32,
    box_w: u32,
    box_h: u32,
    mode: FitMode,
) -> (u32, u32) {
    if wm_w == 0 || wm_h == 0 || box_w == 0 || box_h == 0 {
        return (0, 0);
    }

    let wm_aspect = wm_w as f64 / wm_h as f64;
    let box_aspect = box_w as f64 / box_h as f64;

    match mode {
        FitMode::Distort => (box_w, box_h),

        FitMode::Fit => {
            if wm_aspect > box_aspect {
                let h = (box_w as f64 / wm_aspect).round() as u32;
                (box_w, h.max(1))
            } else {
                let w = (box_h as f64 * wm_aspect).round() as u32;
                (w.max(1), box_h)
            }
        }

        FitMode::Within => {
            if wm_w <= box_w && wm_h <= box_h {
                (wm_w, wm_h)
            } else if wm_aspect > box_aspect {
                let h = (box_w as f64 / wm_aspect).round() as u32;
                (box_w, h.max(1))
            } else {
                let w = (box_h as f64 * wm_aspect).round() as u32;
                (w.max(1), box_h)
            }
        }

        FitMode::FitCrop => {
            if wm_aspect > box_aspect {
                let w = (box_h as f64 * wm_aspect).round() as u32;
                (w.min(box_w).max(1), box_h)
            } else {
                let h = (box_w as f64 / wm_aspect).round() as u32;
                (box_w, h.min(box_h).max(1))
            }
        }

        FitMode::WithinCrop => {
            if wm_w <= box_w && wm_h <= box_h {
                (wm_w, wm_h)
            } else if wm_aspect > box_aspect {
                let w = (box_h as f64 * wm_aspect).round() as u32;
                (w.min(box_w).max(1), box_h.min(wm_h))
            } else {
                let h = (box_w as f64 / wm_aspect).round() as u32;
                (box_w.min(wm_w), h.min(box_h).max(1))
            }
        }
    }
}

// ─── Gravity positioning ───

fn compute_gravity_position(
    box_x1: i32,
    box_y1: i32,
    box_x2: i32,
    box_y2: i32,
    wm_w: i32,
    wm_h: i32,
    gravity: Gravity,
) -> (i32, i32) {
    let (gx, gy) = match gravity {
        Gravity::Center => (50.0f32, 50.0f32),
        Gravity::Percentage(x, y) => (x, y),
    };

    let box_w = box_x2 - box_x1;
    let box_h = box_y2 - box_y1;

    let x = if box_w > wm_w {
        box_x1 + ((box_w - wm_w) as f32 * gx.clamp(0.0, 100.0) / 100.0).round() as i32
    } else {
        box_x1
    };
    let y = if box_h > wm_h {
        box_y1 + ((box_h - wm_h) as f32 * gy.clamp(0.0, 100.0) / 100.0).round() as i32
    } else {
        box_y1
    };

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_canvas_center() {
        let layout = WatermarkLayout {
            wm_width: 100,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(1000, 800).unwrap();
        // 100x50 fits within 1000x800 without scaling.
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 50);
        // Centered in 1000x800.
        assert_eq!(p.x, 450); // (1000-100)/2
        assert_eq!(p.y, 375); // (800-50)/2
    }

    #[test]
    fn margins_bottom_right() {
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::Margins {
                left: 10,
                top: 10,
                right: 10,
                bottom: 10,
            },
            fit_mode: FitMode::Within,
            gravity: Gravity::Percentage(100.0, 100.0),
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        assert_eq!(p.width, 50);
        assert_eq!(p.height, 50);
        // Bottom-right of the margin box (10..190 = 180px).
        assert_eq!(p.x, 140); // 10 + (180-50)
        assert_eq!(p.y, 140);
    }

    #[test]
    fn min_canvas_too_small() {
        let layout = WatermarkLayout {
            wm_width: 100,
            wm_height: 100,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: Some(500),
            min_canvas_height: None,
        };
        assert!(layout.resolve(400, 800).is_none());
    }

    #[test]
    fn fit_scales_up() {
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 25,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Fit,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        // 2:1 aspect, fit into 200x200 → 200x100.
        assert_eq!(p.width, 200);
        assert_eq!(p.height, 100);
    }

    #[test]
    fn within_no_upscale() {
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 25,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        // Within: doesn't upscale when already fits.
        assert_eq!(p.width, 50);
        assert_eq!(p.height, 25);
    }

    #[test]
    fn percentage_box() {
        let layout = WatermarkLayout {
            wm_width: 100,
            wm_height: 100,
            fit_box: FitBox::Percentage {
                x1: 50.0,
                y1: 50.0,
                x2: 100.0,
                y2: 100.0,
            },
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        // Box is 100x100 (right half, bottom half).
        // 100x100 watermark fits exactly.
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 100);
        assert_eq!(p.x, 100); // centered in 100..200
        assert_eq!(p.y, 100);
    }

    #[test]
    fn distort_mode() {
        let layout = WatermarkLayout {
            wm_width: 100,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Distort,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(300, 200).unwrap();
        // Distort: exact box size.
        assert_eq!(p.width, 300);
        assert_eq!(p.height, 200);
    }

    #[test]
    fn invalid_margins_returns_none() {
        let layout = WatermarkLayout {
            wm_width: 100,
            wm_height: 100,
            fit_box: FitBox::Margins {
                left: 100,
                top: 100,
                right: 100,
                bottom: 100,
            },
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        // Canvas 150x150, margins 100+100 = 200 > 150.
        assert!(layout.resolve(150, 150).is_none());
    }

    #[test]
    fn fitcrop_wide_watermark() {
        // Wide watermark (200x50) into 100x100 box.
        // wm_aspect=4.0 > box_aspect=1.0, so fill height first:
        // w = (100 * 4.0).round() = 400, clipped to min(400, 100) = 100; h = 100.
        let layout = WatermarkLayout {
            wm_width: 200,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::FitCrop,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 100).unwrap();
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 100);
    }

    #[test]
    fn fitcrop_tall_watermark() {
        // Tall watermark (50x200) into 100x100 box.
        // wm_aspect=0.25 < box_aspect=1.0, so fill width first:
        // h = (100 / 0.25).round() = 400, clipped to min(400, 100) = 100; w = 100.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 200,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::FitCrop,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 100).unwrap();
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 100);
    }

    #[test]
    fn withincrop_no_upscale() {
        // Small watermark (30x30) in 100x100 box — already fits, no scaling.
        let layout = WatermarkLayout {
            wm_width: 30,
            wm_height: 30,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::WithinCrop,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 100).unwrap();
        assert_eq!(p.width, 30);
        assert_eq!(p.height, 30);
        // Centered: (100-30)/2 = 35.
        assert_eq!(p.x, 35);
        assert_eq!(p.y, 35);
    }

    #[test]
    fn withincrop_downscale() {
        // Large watermark (400x200) in 100x100 box.
        // wm_aspect=2.0 > box_aspect=1.0:
        //   w = (100 * 2.0).round() = 200, min(200, 100) = 100
        //   h = min(100, 200) = 100
        let layout = WatermarkLayout {
            wm_width: 400,
            wm_height: 200,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::WithinCrop,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 100).unwrap();
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 100);
    }

    #[test]
    fn gravity_top_left() {
        // Percentage(0.0, 0.0) → top-left corner.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Percentage(0.0, 0.0),
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        assert_eq!(p.width, 50);
        assert_eq!(p.height, 50);
        // x = 0 + ((200-50) * 0.0 / 100.0).round() = 0
        assert_eq!(p.x, 0);
        assert_eq!(p.y, 0);
    }

    #[test]
    fn gravity_bottom_right() {
        // Percentage(100.0, 100.0) → bottom-right corner.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Percentage(100.0, 100.0),
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(200, 200).unwrap();
        assert_eq!(p.width, 50);
        assert_eq!(p.height, 50);
        // x = 0 + ((200-50) * 100.0 / 100.0).round() = 150
        assert_eq!(p.x, 150);
        assert_eq!(p.y, 150);
    }

    #[test]
    fn zero_dimension_watermark_returns_none() {
        let layout = WatermarkLayout {
            wm_width: 0,
            wm_height: 100,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        assert!(layout.resolve(200, 200).is_none());
    }

    #[test]
    fn watermark_larger_than_canvas_within_scales_down() {
        // 500x250 watermark, 100x100 canvas/box.
        // wm_aspect=2.0 > box_aspect=1.0 → constrained by width:
        //   w = 100, h = (100 / 2.0).round() = 50.
        let layout = WatermarkLayout {
            wm_width: 500,
            wm_height: 250,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 100).unwrap();
        assert_eq!(p.width, 100);
        assert_eq!(p.height, 50);
        // Centered: x = (100-100)/2 = 0, y = (100-50)/2 = 25.
        assert_eq!(p.x, 0);
        assert_eq!(p.y, 25);
    }

    #[test]
    fn percentage_box_inverted_returns_none() {
        // x2 < x1 → invalid bounding box.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::Percentage {
                x1: 80.0,
                y1: 0.0,
                x2: 20.0,
                y2: 100.0,
            },
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: None,
        };
        // px1 = (80/100 * 200).round() = 160, px2 = (20/100 * 200).round() = 40.
        // 160 < 40 is false → None.
        assert!(layout.resolve(200, 200).is_none());
    }

    #[test]
    fn canvas_exactly_at_minimum() {
        // min_canvas_width=100, canvas width=100 → 100 < 100 is false, proceeds.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: Some(100),
            min_canvas_height: None,
        };
        let p = layout.resolve(100, 200).unwrap();
        assert_eq!(p.width, 50);
        assert_eq!(p.height, 50);
    }

    #[test]
    fn canvas_one_pixel_below_minimum() {
        // min_canvas_width=100, canvas width=99 → 99 < 100 is true, returns None.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: Some(100),
            min_canvas_height: None,
        };
        assert!(layout.resolve(99, 200).is_none());
    }

    #[test]
    fn min_canvas_height_only() {
        // min_canvas_height=500, canvas 1000x400 → 400 < 500 is true, returns None.
        let layout = WatermarkLayout {
            wm_width: 50,
            wm_height: 50,
            fit_box: FitBox::FullCanvas,
            fit_mode: FitMode::Within,
            gravity: Gravity::Center,
            min_canvas_width: None,
            min_canvas_height: Some(500),
        };
        assert!(layout.resolve(1000, 400).is_none());
    }
}
