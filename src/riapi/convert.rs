//! Convert parsed RIAPI [`Instructions`] into a zenlayout [`Pipeline`].

use crate::constraint::{
    CanvasColor, Constraint, ConstraintMode, Gravity, LayoutError, SourceCrop,
};
use crate::float_math::F64Ext;
use crate::orientation::Orientation;
use crate::plan::Pipeline;

use super::instructions::{Anchor1D, FitMode, Instructions, ScaleMode};

impl Instructions {
    /// Build a zenlayout [`Pipeline`] from these instructions.
    ///
    /// `source_w` / `source_h`: original source image dimensions (pre-orientation).
    /// `exif`: EXIF orientation tag (1–8), if known.
    ///
    /// Returns `Err` only if the resulting layout is invalid (e.g. zero dimensions).
    pub fn to_pipeline(
        &self,
        source_w: u32,
        source_h: u32,
        exif: Option<u8>,
    ) -> Result<Pipeline, LayoutError> {
        // Validate float fields for NaN/Inf.
        self.validate_floats()?;

        // ---- 1. Source orientation (autorotate + srotate + sflip) ----
        let mut src_orient = Orientation::Identity;

        if self.autorotate.unwrap_or(true)
            && let Some(exif_val) = exif
            && let Some(o) = Orientation::from_exif(exif_val)
        {
            src_orient = src_orient.compose(o);
        }

        if let Some(srotate) = self.srotate {
            src_orient = src_orient.compose(rotation_to_orient(srotate));
        }

        if let Some((h, v)) = self.sflip {
            if h {
                src_orient = src_orient.compose(Orientation::FlipH);
            }
            if v {
                src_orient = src_orient.compose(Orientation::FlipV);
            }
        }

        // ---- 2. Post-resize orientation (rotate + flip) ----
        let mut post_orient = Orientation::Identity;

        if let Some(rotate) = self.rotate {
            post_orient = post_orient.compose(rotation_to_orient(rotate));
        }

        if let Some((h, v)) = self.flip {
            if h {
                post_orient = post_orient.compose(Orientation::FlipH);
            }
            if v {
                post_orient = post_orient.compose(Orientation::FlipV);
            }
        }

        // Compose post-resize orientation into source orientation.
        // If post-resize rotation swaps axes, we swap target dimensions to compensate.
        let post_swaps = post_orient.swaps_axes();
        let full_orient = src_orient.compose(post_orient);

        // ---- 3. Post-orientation source dimensions (for crop + dimension resolution) ----
        let display = src_orient.transform_dimensions(source_w, source_h);
        let (disp_w, disp_h) = (display.width, display.height);

        // ---- 4. Crop ----
        let source_crop = self.resolve_crop(disp_w, disp_h);

        // Effective dimensions (post-crop) for aspect ratio calculations
        let (eff_w, eff_h) = match &source_crop {
            Some(crop) => {
                let r = crop.resolve(disp_w, disp_h);
                (r.width, r.height)
            }
            None => (disp_w, disp_h),
        };

        // ---- 5. Resolve target dimensions ----
        let (mut target_w, mut target_h) = self.resolve_dimensions(eff_w, eff_h);

        // Apply zoom
        let zoom = self.zoom.unwrap_or(1.0).clamp(0.00008, 80000.0);
        if let Some(w) = &mut target_w {
            *w = (*w as f64 * zoom).round_().clamp(1.0, i32::MAX as f64) as i32;
        }
        if let Some(h) = &mut target_h {
            *h = (*h as f64 * zoom).round_().clamp(1.0, i32::MAX as f64) as i32;
        }

        // If post-resize orientation swaps axes, swap target dimensions
        if post_swaps {
            core::mem::swap(&mut target_w, &mut target_h);
        }

        // ---- 6. Mode inference ----
        let mode = if target_w.is_none() && target_h.is_none() {
            FitMode::Max
        } else {
            self.mode.unwrap_or(FitMode::Pad)
        };
        let scale = self.scale.unwrap_or(ScaleMode::DownscaleOnly);

        // ---- 7. Map mode × scale → ConstraintMode ----
        let constraint_mode = map_mode_scale(mode, scale, target_w, target_h, eff_w, eff_h);

        // ---- 8. Gravity ----
        let gravity = self.resolve_gravity();

        // ---- 9. Background color ----
        let canvas_color = self.bgcolor.unwrap_or(CanvasColor::Transparent);

        // ---- 10. Build pipeline ----
        let mut pipeline = Pipeline::new(source_w, source_h);

        // Apply full orientation
        if !full_orient.is_identity() {
            // Apply as EXIF value
            pipeline = pipeline.auto_orient(full_orient.to_exif());
        }

        // Apply crop
        if let Some(crop) = source_crop {
            pipeline = pipeline.crop(crop);
        }

        // For Crop+UpscaleCanvas mixed case: clip overflowing dimension.
        // When one source dim exceeds target and the other doesn't, we crop
        // the excess to target and let PadWithin handle the smaller dimension.
        if mode == FitMode::Crop
            && scale == ScaleMode::UpscaleCanvas
            && let (Some(tw_i), Some(th_i)) = (target_w, target_h)
        {
            let tw = tw_i as u32;
            let th = th_i as u32;
            let mixed = (eff_w > tw) != (eff_h > th); // exactly one exceeds
            if mixed {
                let clip_w = eff_w.min(tw);
                let clip_h = eff_h.min(th);
                // Use percent-based crop with gravity to clip the excess dimension
                let fx = if eff_w > clip_w {
                    match gravity {
                        Gravity::Center => (eff_w - clip_w) as f32 / (2.0 * eff_w as f32),
                        Gravity::Percentage(x, _) => {
                            (eff_w - clip_w) as f32 * x / eff_w as f32
                        }
                    }
                } else {
                    0.0
                };
                let fy = if eff_h > clip_h {
                    match gravity {
                        Gravity::Center => (eff_h - clip_h) as f32 / (2.0 * eff_h as f32),
                        Gravity::Percentage(_, y) => {
                            (eff_h - clip_h) as f32 * y / eff_h as f32
                        }
                    }
                } else {
                    0.0
                };
                let fw = clip_w as f32 / eff_w as f32;
                let fh = clip_h as f32 / eff_h as f32;
                pipeline = pipeline.crop(SourceCrop::percent(fx, fy, fw, fh));
            }
        }

        // Apply constraint (if we have target dimensions)
        if let Some(cm) = constraint_mode {
            let constraint = match (target_w, target_h) {
                (Some(w), Some(h)) => Constraint::new(cm, w as u32, h as u32)
                    .gravity(gravity)
                    .canvas_color(canvas_color),
                (Some(w), None) => Constraint::width_only(cm, w as u32)
                    .gravity(gravity)
                    .canvas_color(canvas_color),
                (None, Some(h)) => Constraint::height_only(cm, h as u32)
                    .gravity(gravity)
                    .canvas_color(canvas_color),
                (None, None) => return Ok(pipeline), // No constraint needed
            };
            pipeline = pipeline.constrain(constraint);
        }

        Ok(pipeline)
    }

    /// Resolve crop from RIAPI crop parameters to a SourceCrop.
    fn resolve_crop(&self, display_w: u32, display_h: u32) -> Option<SourceCrop> {
        let crop = self.crop.as_ref()?;
        let [x1, y1, x2, y2] = *crop;

        let xu = match self.cropxunits {
            Some(v) if v > 0.0 => v,
            _ => display_w as f64,
        };
        let yu = match self.cropyunits {
            Some(v) if v > 0.0 => v,
            _ => display_h as f64,
        };

        // Convert to fractions of source dimensions
        let fx1 = x1 / xu;
        let fy1 = y1 / yu;
        let fx2 = x2 / xu;
        let fy2 = y2 / yu;

        // Handle negative offsets (from far edge)
        let fx1 = if fx1 < 0.0 { fx1 + 1.0 } else { fx1 };
        let fy1 = if fy1 < 0.0 { fy1 + 1.0 } else { fy1 };
        let fx2 = if fx2 <= 0.0 { fx2 + 1.0 } else { fx2 };
        let fy2 = if fy2 <= 0.0 { fy2 + 1.0 } else { fy2 };

        let x = fx1.clamp(0.0, 1.0) as f32;
        let y = fy1.clamp(0.0, 1.0) as f32;
        let w = (fx2 - fx1).clamp(0.0, 1.0 - x as f64) as f32;
        let h = (fy2 - fy1).clamp(0.0, 1.0 - y as f64) as f32;

        if w <= 0.0 || h <= 0.0 {
            return None;
        }

        Some(SourceCrop::percent(x, y, w, h))
    }

    /// Resolve target dimensions using imageflow_riapi's `get_wh_from_all` algorithm.
    ///
    /// Returns `(Option<w>, Option<h>)` — either or both may be `None`.
    fn resolve_dimensions(&self, source_w: u32, source_h: u32) -> (Option<i32>, Option<i32>) {
        let mut w = self.w.unwrap_or(-1).max(-1);
        let mut h = self.h.unwrap_or(-1).max(-1);
        let mut mw = self.legacy_max_width.unwrap_or(-1).max(-1);
        let mut mh = self.legacy_max_height.unwrap_or(-1).max(-1);

        // When both value and max are specified, use the smaller
        if mw > 0 && w > 0 {
            w = w.min(mw);
            mw = -1;
        }
        if mh > 0 && h > 0 {
            h = h.min(mh);
            mh = -1;
        }

        let (sw, sh) = (source_w as f64, source_h as f64);

        // Cross-dimension constraints
        if w > 0 && mh > 0 && sw > 0.0 {
            let aspect_h = (w as f64 * sh / sw).round_() as i32;
            if aspect_h > 0 {
                mh = mh.min(aspect_h);
            }
        }
        if h > 0 && mw > 0 && sh > 0.0 {
            let aspect_w = (h as f64 * sw / sh).round_() as i32;
            if aspect_w > 0 {
                mw = mw.min(aspect_w);
            }
        }

        // Merge max values into w/h
        w = w.max(mw);
        h = h.max(mh);

        let rw = if w < 1 { None } else { Some(w) };
        let rh = if h < 1 { None } else { Some(h) };
        (rw, rh)
    }

    /// Resolve gravity from c.gravity, anchor, or default Center.
    fn resolve_gravity(&self) -> Gravity {
        // c.gravity takes priority over anchor
        if let Some([x, y]) = self.c_gravity {
            return Gravity::Percentage(
                (x as f32 / 100.0).clamp(0.0, 1.0),
                (y as f32 / 100.0).clamp(0.0, 1.0),
            );
        }

        if let Some((ax, ay)) = &self.anchor {
            let x = anchor1d_to_fraction(ax);
            let y = anchor1d_to_fraction(ay);
            return Gravity::Percentage(x, y);
        }

        Gravity::Center
    }
}

/// Convert an Anchor1D to a 0.0–1.0 fraction.
fn anchor1d_to_fraction(a: &Anchor1D) -> f32 {
    match a {
        Anchor1D::Near => 0.0,
        Anchor1D::Center => 0.5,
        Anchor1D::Far => 1.0,
        Anchor1D::Percent(p) => (*p / 100.0).clamp(0.0, 1.0),
    }
}

/// Map rotation degrees (0/90/180/270) to an Orientation.
fn rotation_to_orient(degrees: i32) -> Orientation {
    match degrees {
        90 => Orientation::Rotate90,
        180 => Orientation::Rotate180,
        270 => Orientation::Rotate270,
        _ => Orientation::Identity,
    }
}

/// Map RIAPI FitMode × ScaleMode to zenlayout ConstraintMode.
///
/// Returns `None` when no constraint should be applied (identity).
fn map_mode_scale(
    mode: FitMode,
    scale: ScaleMode,
    target_w: Option<i32>,
    target_h: Option<i32>,
    source_w: u32,
    source_h: u32,
) -> Option<ConstraintMode> {
    // No target dimensions → no constraint
    if target_w.is_none() && target_h.is_none() {
        return None;
    }

    let tw = target_w.unwrap_or(i32::MAX) as u32;
    let th = target_h.unwrap_or(i32::MAX) as u32;
    let source_fits = source_w <= tw && source_h <= th;
    let source_exceeds = source_w > tw || source_h > th;

    // UpscaleOnly: only operate when source fits within target in both dims.
    // Otherwise identity (skip).
    if scale == ScaleMode::UpscaleOnly && mode != FitMode::AspectCrop {
        if !source_fits {
            return None; // Source exceeds target on at least one dim → identity
        }
        // Source fits → apply the "Both" variant (upscale to target)
        return match mode {
            FitMode::Max => Some(ConstraintMode::Fit),
            FitMode::Pad => Some(ConstraintMode::FitPad),
            FitMode::Crop => Some(ConstraintMode::FitCrop),
            FitMode::Stretch => Some(ConstraintMode::Distort),
            FitMode::AspectCrop => unreachable!(),
        };
    }

    // UpscaleCanvas: never upscale the image, but always provide target canvas.
    if scale == ScaleMode::UpscaleCanvas && mode != FitMode::AspectCrop {
        return match mode {
            // Max/Pad: downscale to fit if needed, always pad to target canvas.
            FitMode::Max | FitMode::Pad => Some(ConstraintMode::PadWithin),

            // Crop: if source exceeds both dims, crop+downscale normally.
            // Otherwise PadWithin (mixed case gets a pre-crop in to_pipeline).
            FitMode::Crop => {
                if source_w >= tw && source_h >= th {
                    Some(ConstraintMode::WithinCrop)
                } else {
                    Some(ConstraintMode::PadWithin)
                }
            }

            // Stretch: distort if source exceeds, otherwise pad without upscale.
            FitMode::Stretch => {
                if source_exceeds {
                    Some(ConstraintMode::Distort)
                } else {
                    Some(ConstraintMode::PadWithin)
                }
            }

            FitMode::AspectCrop => unreachable!(),
        };
    }

    // DownscaleOnly + Stretch: only distort when source exceeds target
    if mode == FitMode::Stretch && scale == ScaleMode::DownscaleOnly && source_fits {
        return None;
    }

    match (mode, scale) {
        // AspectCrop ignores scale mode
        (FitMode::AspectCrop, _) => Some(ConstraintMode::AspectCrop),

        // Max (proportional fit, no padding)
        (FitMode::Max, ScaleMode::DownscaleOnly) => Some(ConstraintMode::Within),
        (FitMode::Max, ScaleMode::Both) => Some(ConstraintMode::Fit),

        // Pad (proportional fit + pad to exact target)
        (FitMode::Pad, ScaleMode::DownscaleOnly) => Some(ConstraintMode::WithinPad),
        (FitMode::Pad, ScaleMode::Both) => Some(ConstraintMode::FitPad),

        // Crop (proportional fill + crop overflow)
        (FitMode::Crop, ScaleMode::DownscaleOnly) => Some(ConstraintMode::WithinCrop),
        (FitMode::Crop, ScaleMode::Both) => Some(ConstraintMode::FitCrop),

        // Stretch (distort to exact target)
        (FitMode::Stretch, ScaleMode::DownscaleOnly | ScaleMode::Both) => {
            Some(ConstraintMode::Distort)
        }

        // UpscaleOnly/UpscaleCanvas handled above
        (_, ScaleMode::UpscaleOnly | ScaleMode::UpscaleCanvas) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Size;

    /// Helper: parse query, build pipeline, plan it, return resize_to dimensions.
    fn query_to_resize(query: &str, sw: u32, sh: u32) -> Size {
        let result = crate::riapi::parse(query);
        let pipeline = result
            .instructions
            .to_pipeline(sw, sh, None)
            .expect("pipeline should succeed");
        let (ideal, _) = pipeline.plan().expect("plan should succeed");
        ideal.layout.resize_to
    }

    /// Helper: parse query, build pipeline, plan it, return canvas dimensions.
    fn query_to_canvas(query: &str, sw: u32, sh: u32) -> Size {
        let result = crate::riapi::parse(query);
        let pipeline = result
            .instructions
            .to_pipeline(sw, sh, None)
            .expect("pipeline should succeed");
        let (ideal, _) = pipeline.plan().expect("plan should succeed");
        ideal.layout.canvas
    }

    #[test]
    fn default_mode_is_pad_scale_is_down() {
        // ?w=800&h=600 with 1000x500 source
        // Default: mode=pad, scale=down → WithinPad
        // 1000x500 into 800x600: fits at 800x400, canvas 800x600
        let canvas = query_to_canvas("w=800&h=600", 1000, 500);
        assert_eq!(canvas, Size::new(800, 600));
        let resize = query_to_resize("w=800&h=600", 1000, 500);
        assert_eq!(resize, Size::new(800, 400));
    }

    #[test]
    fn mode_max_scale_down() {
        // Within: fit proportionally, never upscale
        let resize = query_to_resize("w=800&h=600&mode=max", 1000, 500);
        // 1000x500 into 800x600: width-limited → 800x400
        assert_eq!(resize, Size::new(800, 400));
    }

    #[test]
    fn mode_max_scale_down_no_upscale() {
        // Small source: 200x100 into 800x600 with mode=max (Within) → no upscale
        let resize = query_to_resize("w=800&h=600&mode=max", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
    }

    #[test]
    fn mode_max_scale_both() {
        // Fit: always scale to fit within target
        let resize = query_to_resize("w=800&h=600&mode=max&scale=both", 200, 100);
        // 200x100 (2:1) into 800x600: width-limited → 800x400
        assert_eq!(resize, Size::new(800, 400));
    }

    #[test]
    fn mode_crop_scale_both() {
        // FitCrop: fill target, crop overflow
        let resize = query_to_resize("w=800&h=600&mode=crop&scale=both", 1000, 500);
        // 1000x500 (2:1) into 800x600 (4:3): height-limited → 1200x600, crop to 800x600
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn mode_crop_scale_down() {
        // WithinCrop: crop without upscaling
        let resize = query_to_resize("w=400&h=300&mode=crop", 1000, 500);
        assert_eq!(resize, Size::new(400, 300));
    }

    #[test]
    fn mode_stretch() {
        // Distort to exact dimensions
        let resize = query_to_resize("w=800&h=600&mode=stretch&scale=both", 1000, 500);
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn single_dim_width_only() {
        // Width-only: height derived from aspect ratio
        let resize = query_to_resize("w=500&mode=max", 1000, 500);
        assert_eq!(resize, Size::new(500, 250));
    }

    #[test]
    fn single_dim_height_only() {
        let resize = query_to_resize("h=250&mode=max", 1000, 500);
        assert_eq!(resize, Size::new(500, 250));
    }

    #[test]
    fn zoom_multiplier() {
        // zoom=2 doubles the target
        let resize = query_to_resize("w=400&h=300&mode=max&scale=both&zoom=2", 1000, 500);
        // Target becomes 800x600. 1000x500 fit → 800x400
        assert_eq!(resize, Size::new(800, 400));
    }

    #[test]
    fn maxwidth_constrains_w() {
        // w=800, maxwidth=500 → effective w=500
        let resize = query_to_resize("w=800&maxwidth=500&mode=max", 1000, 500);
        assert_eq!(resize, Size::new(500, 250));
    }

    #[test]
    fn maxheight_constrains_h() {
        // h=800, maxheight=300 → effective h=300
        // Source 1000x500 (2:1), Within height=300 → 600x300
        let resize = query_to_resize("h=800&maxheight=300&mode=max", 1000, 500);
        assert_eq!(resize, Size::new(600, 300));
    }

    #[test]
    fn crop_c_percent() {
        // c=10,10,90,90 → crop center 80% then constrain
        let resize = query_to_resize("w=400&h=300&mode=max&scale=both&c=10,10,90,90", 1000, 500);
        // Crop: 10-90% of 1000x500 = 800x400
        // 800x400 (2:1) fit 400x300: width-limited → 400x200
        assert_eq!(resize, Size::new(400, 200));
    }

    #[test]
    fn bgcolor_pad() {
        let result = crate::riapi::parse("w=800&h=600&bgcolor=ff0000");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // Canvas should be 800x600 with red background
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert_eq!(
            ideal.layout.canvas_color,
            CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
    }

    #[test]
    fn srotate_90_swaps_source() {
        // Source 1000x500, srotate=90 → effective 500x1000
        // w=800&h=600 mode=max → 500x1000 into 800x600: h-limited → 300x600
        let resize = query_to_resize("w=800&h=600&mode=max&srotate=90", 1000, 500);
        assert_eq!(resize, Size::new(300, 600));
    }

    #[test]
    fn post_rotate_swaps_target() {
        // Source 1000x500, rotate=90
        // Target w=800, h=600 → swapped to w=600, h=800 for constraint
        // Full orient = Rotate90, effective source 500x1000
        // 500x1000 fit 600x800: w-limited → 400x800... let's verify
        // Actually: 500x1000 (1:2), target 600x800 (3:4)
        // Width-limited: 600 → 600x1200 (too tall)
        // Height-limited: 800 → 400x800
        let resize = query_to_resize("w=800&h=600&mode=max&scale=both&rotate=90", 1000, 500);
        assert_eq!(resize, Size::new(400, 800));
    }

    #[test]
    fn autorotate_with_exif() {
        // Source 500x1000 with EXIF 6 (Rotate90) → display 1000x500
        // w=800&h=600 mode=max → 1000x500 into 800x600 → 800x400
        let resize = query_to_resize("w=800&h=600&mode=max", 500, 1000);
        // No EXIF: 500x1000 into 800x600 → h-limited: 300x600
        assert_eq!(resize, Size::new(300, 600));

        // With EXIF 6:
        let result = crate::riapi::parse("w=800&h=600&mode=max");
        let pipeline = result.instructions.to_pipeline(500, 1000, Some(6)).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // EXIF 6 = Rotate90: 500x1000 → display 1000x500
        // 1000x500 into 800x600 → 800x400
        assert_eq!(ideal.layout.resize_to, Size::new(800, 400));
    }

    #[test]
    fn autorotate_false_ignores_exif() {
        let result = crate::riapi::parse("w=800&h=600&mode=max&autorotate=false");
        let pipeline = result.instructions.to_pipeline(500, 1000, Some(6)).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // EXIF ignored: source stays 500x1000
        // 500x1000 into 800x600 → h-limited: 300x600
        assert_eq!(ideal.layout.resize_to, Size::new(300, 600));
    }

    #[test]
    fn no_dimensions_identity() {
        // No w, h, maxwidth, maxheight → force Max mode, identity resize
        let resize = query_to_resize("mode=crop", 1000, 500);
        assert_eq!(resize, Size::new(1000, 500));
    }

    #[test]
    fn stretch_downscale_only_small_source() {
        // Stretch+DownscaleOnly: source 200x100 fits in 800x600 → identity
        let resize = query_to_resize("w=800&h=600&mode=stretch", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
    }

    #[test]
    fn stretch_downscale_only_large_source() {
        // Stretch+DownscaleOnly: source exceeds target → distort
        let resize = query_to_resize("w=800&h=600&mode=stretch", 1000, 1000);
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn aspect_crop_mode() {
        let resize = query_to_resize("w=400&h=400&mode=aspectcrop", 1000, 500);
        // AspectCrop: crop to 1:1 from 1000x500 → 500x500 (no scaling)
        assert_eq!(resize, Size::new(500, 500));
    }

    #[test]
    fn anchor_topleft_crop() {
        let result = crate::riapi::parse("w=400&h=300&mode=crop&anchor=topleft&scale=both");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // Crop from top-left: crop offset should be (0, 0)
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        if let Some(crop) = ideal.layout.source_crop {
            assert_eq!(crop.x, 0);
            assert_eq!(crop.y, 0);
        }
    }

    #[test]
    fn gravity_overrides_anchor() {
        let result =
            crate::riapi::parse("w=400&h=300&mode=crop&anchor=topleft&c.gravity=50,50&scale=both");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // c.gravity should override anchor → center crop
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
    }

    // ---- UpscaleOnly tests ----

    #[test]
    fn upscale_only_max_small_source() {
        // Source 200x100, target 800x600: source fits → upscale (Fit)
        let resize = query_to_resize("w=800&h=600&mode=max&scale=up", 200, 100);
        // 200x100 (2:1) fit 800x600 → 800x400
        assert_eq!(resize, Size::new(800, 400));
    }

    #[test]
    fn upscale_only_max_large_source() {
        // Source 1000x500, target 800x600: source exceeds → identity
        let resize = query_to_resize("w=800&h=600&mode=max&scale=up", 1000, 500);
        assert_eq!(resize, Size::new(1000, 500));
    }

    #[test]
    fn upscale_only_pad_small_source() {
        // Source 200x100, target 800x600: upscale + pad
        let resize = query_to_resize("w=800&h=600&mode=pad&scale=up", 200, 100);
        // 200x100 (2:1) fit 800x600 → 800x400 resize, 800x600 canvas
        assert_eq!(resize, Size::new(800, 400));
        let canvas = query_to_canvas("w=800&h=600&mode=pad&scale=up", 200, 100);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn upscale_only_pad_large_source() {
        // Source 1000x500, target 800x600: source exceeds → identity
        let resize = query_to_resize("w=800&h=600&mode=pad&scale=up", 1000, 500);
        assert_eq!(resize, Size::new(1000, 500));
    }

    #[test]
    fn upscale_only_crop_small_source() {
        // Source 200x100, target 800x600: upscale + crop (FitCrop)
        let resize = query_to_resize("w=400&h=300&mode=crop&scale=up", 200, 100);
        // 200x100 (2:1) fill 400x300 (4:3): h-limited → 600x300, crop to 400x300
        assert_eq!(resize, Size::new(400, 300));
    }

    #[test]
    fn upscale_only_crop_large_source() {
        // Source 1000x500, target 400x300: source exceeds → identity
        let resize = query_to_resize("w=400&h=300&mode=crop&scale=up", 1000, 500);
        assert_eq!(resize, Size::new(1000, 500));
    }

    #[test]
    fn upscale_only_stretch_small_source() {
        // Source 200x100, target 800x600: distort to target
        let resize = query_to_resize("w=800&h=600&mode=stretch&scale=up", 200, 100);
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn upscale_only_stretch_large_source() {
        // Source 1000x500, target 800x600: source exceeds → identity
        let resize = query_to_resize("w=800&h=600&mode=stretch&scale=up", 1000, 500);
        assert_eq!(resize, Size::new(1000, 500));
    }

    // ---- UpscaleCanvas tests ----

    #[test]
    fn canvas_max_small_source() {
        // Source 200x100, target 800x600: no upscale, pad to canvas
        let resize = query_to_resize("w=800&h=600&mode=max&scale=canvas", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
        let canvas = query_to_canvas("w=800&h=600&mode=max&scale=canvas", 200, 100);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_max_large_source() {
        // Source 1000x500, target 800x600: downscale to fit, pad to canvas
        let resize = query_to_resize("w=800&h=600&mode=max&scale=canvas", 1000, 500);
        assert_eq!(resize, Size::new(800, 400));
        let canvas = query_to_canvas("w=800&h=600&mode=max&scale=canvas", 1000, 500);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_pad_small_source() {
        // Source 200x100, target 800x600: no upscale, pad to canvas
        let resize = query_to_resize("w=800&h=600&mode=pad&scale=canvas", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
        let canvas = query_to_canvas("w=800&h=600&mode=pad&scale=canvas", 200, 100);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_pad_large_source() {
        // Source 1000x500, target 800x600: downscale + pad
        let resize = query_to_resize("w=800&h=600&mode=pad&scale=canvas", 1000, 500);
        assert_eq!(resize, Size::new(800, 400));
        let canvas = query_to_canvas("w=800&h=600&mode=pad&scale=canvas", 1000, 500);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_stretch_small_source() {
        // Source 200x100, target 800x600: no distort, pad to canvas
        let resize = query_to_resize("w=800&h=600&mode=stretch&scale=canvas", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
        let canvas = query_to_canvas("w=800&h=600&mode=stretch&scale=canvas", 200, 100);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_stretch_large_source() {
        // Source 1000x500, target 800x600: distort to target
        let resize = query_to_resize("w=800&h=600&mode=stretch&scale=canvas", 1000, 500);
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn canvas_crop_small_source() {
        // Source 200x100, target 800x600: no scale, pad to canvas
        let resize = query_to_resize("w=800&h=600&mode=crop&scale=canvas", 200, 100);
        assert_eq!(resize, Size::new(200, 100));
        let canvas = query_to_canvas("w=800&h=600&mode=crop&scale=canvas", 200, 100);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_crop_large_source() {
        // Source 1000x500, target 800x600: both exceed → WithinCrop
        // 1000x500 into 800x600: crop to 4:3 = 750x500, wait no...
        // WithinCrop: both dims exceed target → crop to aspect + downscale
        // 1000x500 → crop to 4:3: width=667, height=500 → downscale to 800x600?
        // Actually: crop to 800x600 aspect (4:3) from 1000x500 (2:1)
        // source is wider → crop width to 500*4/3=667. Downscale to 800x600.
        let resize = query_to_resize("w=800&h=600&mode=crop&scale=canvas", 1000, 500);
        // 1000x500 fits within 800x600? No, 1000>800. But 500<600. So mixed.
        // Mixed case: clip width to 800, keep height 500. Pad to 800x600.
        assert_eq!(resize, Size::new(800, 500));
        let canvas = query_to_canvas("w=800&h=600&mode=crop&scale=canvas", 1000, 500);
        assert_eq!(canvas, Size::new(800, 600));
    }

    #[test]
    fn canvas_crop_both_exceed() {
        // Source 1200x900, target 800x600: both exceed → WithinCrop → exactly target
        let resize = query_to_resize("w=800&h=600&mode=crop&scale=canvas", 1200, 900);
        assert_eq!(resize, Size::new(800, 600));
    }

    #[test]
    fn canvas_crop_mixed_height_exceeds() {
        // Source 400x1000, target 800x600: width fits, height exceeds
        // Clip height to 600 → 400x600. PadWithin: 400x600 fits within 800x600 → pad
        let resize = query_to_resize("w=800&h=600&mode=crop&scale=canvas", 400, 1000);
        assert_eq!(resize, Size::new(400, 600));
        let canvas = query_to_canvas("w=800&h=600&mode=crop&scale=canvas", 400, 1000);
        assert_eq!(canvas, Size::new(800, 600));
    }
}
