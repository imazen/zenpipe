//! Region model — detail view crop region, zoom, and pan math.
//!
//! All coordinates are normalized (0..1) relative to the source image.
//! The view layer converts between pixel/CSS coordinates and normalized
//! coordinates before sending commands.

use serde::{Deserialize, Serialize};

/// The detail view region — a normalized rectangle over the source image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionModel {
    /// Top-left X (0..1).
    pub x: f32,
    /// Top-left Y (0..1).
    pub y: f32,
    /// Width (0..1).
    pub w: f32,
    /// Height (0..1).
    pub h: f32,
    /// Source image dimensions (for clamping).
    pub source_width: u32,
    pub source_height: u32,
}

/// Minimum region size as a fraction of the source (prevents infinite zoom).
const MIN_REGION: f32 = 0.01;

impl Default for RegionModel {
    fn default() -> Self {
        Self {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
            source_width: 0,
            source_height: 0,
        }
    }
}

impl RegionModel {
    /// Set source dimensions (called when a new image is loaded).
    pub fn set_source_dims(&mut self, width: u32, height: u32) {
        self.source_width = width;
        self.source_height = height;
    }

    /// Set the region directly. Clamps to valid range.
    pub fn set(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.w = w.clamp(MIN_REGION, 1.0);
        self.h = h.clamp(MIN_REGION, 1.0);
        self.x = x.clamp(0.0, 1.0 - self.w);
        self.y = y.clamp(0.0, 1.0 - self.h);
    }

    /// Pan by a normalized delta. Clamps to source bounds.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.x = (self.x - dx).clamp(0.0, 1.0 - self.w);
        self.y = (self.y - dy).clamp(0.0, 1.0 - self.h);
    }

    /// Zoom by a factor around a normalized center point.
    ///
    /// `factor > 1.0` zooms in (smaller region), `factor < 1.0` zooms out.
    /// The center point stays fixed in the viewport.
    pub fn zoom(&mut self, factor: f32, center_x: f32, center_y: f32) {
        let new_w = (self.w / factor).clamp(MIN_REGION, 1.0);
        let new_h = (self.h / factor).clamp(MIN_REGION, 1.0);

        // Keep the center point fixed: adjust x,y so the same point in
        // the source is at the same position in the viewport.
        let cx = self.x + center_x * self.w;
        let cy = self.y + center_y * self.h;

        self.w = new_w;
        self.h = new_h;
        self.x = (cx - center_x * new_w).clamp(0.0, 1.0 - new_w);
        self.y = (cy - center_y * new_h).clamp(0.0, 1.0 - new_h);
    }

    /// Reset to show the full image.
    pub fn reset_to_full(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
        self.w = 1.0;
        self.h = 1.0;
    }

    /// Reset to 1:1 pixel ratio (one source pixel = one device pixel).
    ///
    /// `viewport_w/h` are in CSS pixels, `dpr` is `devicePixelRatio`.
    /// The region is sized so that the viewport displays exactly
    /// `viewport_w * dpr` source pixels wide.
    pub fn reset_to_1to1(&mut self, viewport_w: f32, viewport_h: f32, dpr: f32) {
        if self.source_width == 0 || self.source_height == 0 {
            return;
        }

        let device_w = viewport_w * dpr;
        let device_h = viewport_h * dpr;

        let new_w = (device_w / self.source_width as f32).clamp(MIN_REGION, 1.0);
        let new_h = (device_h / self.source_height as f32).clamp(MIN_REGION, 1.0);

        // Center the region
        let cx = self.x + self.w / 2.0;
        let cy = self.y + self.h / 2.0;

        self.w = new_w;
        self.h = new_h;
        self.x = (cx - new_w / 2.0).clamp(0.0, 1.0 - new_w);
        self.y = (cy - new_h / 2.0).clamp(0.0, 1.0 - new_h);
    }

    /// Source pixels covered by this region.
    pub fn source_pixels(&self) -> (u32, u32) {
        let w = (self.w * self.source_width as f32).round().max(1.0) as u32;
        let h = (self.h * self.source_height as f32).round().max(1.0) as u32;
        (w, h)
    }

    /// Convert to absolute pixel coordinates for the pipeline crop node.
    pub fn to_crop_pixels(&self) -> (u32, u32, u32, u32) {
        let x = (self.x * self.source_width as f32) as u32;
        let y = (self.y * self.source_height as f32) as u32;
        let w = (self.w * self.source_width as f32).max(1.0) as u32;
        let h = (self.h * self.source_height as f32).max(1.0) as u32;
        (x, y, w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_clamps_to_bounds() {
        let mut r = RegionModel {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
            source_width: 100,
            source_height: 100,
        };
        // Pan right (negative dx since pan subtracts)
        r.pan(-0.3, 0.0);
        assert!((r.x - 0.3).abs() < 1e-6);
        // Pan too far
        r.pan(-0.9, 0.0);
        assert!((r.x - 0.5).abs() < 1e-6); // clamped to 1.0 - 0.5
    }

    #[test]
    fn zoom_in_shrinks_region() {
        let mut r = RegionModel {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
            source_width: 1000,
            source_height: 1000,
        };
        r.zoom(2.0, 0.5, 0.5); // zoom 2x at center
        assert!(r.w < 0.5);
        assert!(r.h < 0.5);
    }

    #[test]
    fn zoom_out_grows_region() {
        let mut r = RegionModel {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
            source_width: 1000,
            source_height: 1000,
        };
        r.zoom(0.5, 0.5, 0.5); // zoom out
        assert!(r.w > 0.5);
    }

    #[test]
    fn reset_1to1() {
        let mut r = RegionModel {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            source_width: 4000,
            source_height: 3000,
        };
        // 800×600 viewport at 2x DPR = 1600×1200 device pixels
        r.reset_to_1to1(800.0, 600.0, 2.0);
        // Region should be 1600/4000 = 0.4 wide
        assert!((r.w - 0.4).abs() < 1e-3);
    }
}
