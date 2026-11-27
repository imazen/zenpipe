//! Regression tests for smart_crop focus rectangle handling.

#[cfg(feature = "smart-crop")]
mod focus_smart_crop {
    use zenlayout::Rect;
    use zenlayout::smart_crop::*;

    /// Helper to check that a focus region (in percentage coords) is inside
    /// a crop rectangle (in pixel coords).
    fn focus_inside_crop(focus: &FocusRect, crop: &Rect, src_w: u32, src_h: u32) -> bool {
        let fx1 = focus.x1 as f64 / 100.0 * src_w as f64;
        let fy1 = focus.y1 as f64 / 100.0 * src_h as f64;
        let fx2 = focus.x2 as f64 / 100.0 * src_w as f64;
        let fy2 = focus.y2 as f64 / 100.0 * src_h as f64;

        let cx1 = crop.x as f64;
        let cy1 = crop.y as f64;
        let cx2 = (crop.x + crop.width) as f64;
        let cy2 = (crop.y + crop.height) as f64;

        // Check that at least the center of the focus region is inside the crop
        let fcx = (fx1 + fx2) / 2.0;
        let fcy = (fy1 + fy2) / 2.0;

        fcx >= cx1 && fcx <= cx2 && fcy >= cy1 && fcy <= cy2
    }

    /// Fraction of the focus region area that overlaps the crop.
    fn overlap_fraction(focus: &FocusRect, crop: &Rect, src_w: u32, src_h: u32) -> f64 {
        let fx1 = focus.x1 as f64 / 100.0 * src_w as f64;
        let fy1 = focus.y1 as f64 / 100.0 * src_h as f64;
        let fx2 = focus.x2 as f64 / 100.0 * src_w as f64;
        let fy2 = focus.y2 as f64 / 100.0 * src_h as f64;

        let area = (fx2 - fx1) * (fy2 - fy1);
        if area < 1e-10 {
            return 1.0;
        }

        let cx1 = crop.x as f64;
        let cy1 = crop.y as f64;
        let cx2 = (crop.x + crop.width) as f64;
        let cy2 = (crop.y + crop.height) as f64;

        let ox1 = fx1.max(cx1);
        let oy1 = fy1.max(cy1);
        let ox2 = fx2.min(cx2);
        let oy2 = fy2.min(cy2);
        let overlap = (ox2 - ox1).max(0.0) * (oy2 - oy1).max(0.0);
        overlap / area
    }

    // ── FocusRect with specific coordinates keeps focus area visible ──

    #[test]
    fn focus_rect_center_kept_visible_in_crop() {
        // Face centered at (50%, 50%) on a 1920x1080 image.
        // Crop to 9:16 portrait — the face center should remain in the crop.
        let face = FocusRect {
            x1: 40.0,
            y1: 35.0,
            x2: 60.0,
            y2: 65.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![face],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: PORTRAIT_9_16,
            mode: CropMode::Minimal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1920, 1080, &config).unwrap();

        assert!(
            crop.x + crop.width <= 1920,
            "crop right edge exceeds source"
        );
        assert!(
            crop.y + crop.height <= 1080,
            "crop bottom edge exceeds source"
        );
        assert!(
            focus_inside_crop(&face, &crop, 1920, 1080),
            "face center should be inside the crop: crop={crop:?}"
        );

        let frac = overlap_fraction(&face, &crop, 1920, 1080);
        assert!(
            frac >= 0.5,
            "at least 50% of the focus region should be visible: overlap={frac:.4}"
        );
    }

    #[test]
    fn focus_rect_corner_kept_visible() {
        // Face in the bottom-right corner (80-95%, 70-90%).
        let face = FocusRect {
            x1: 80.0,
            y1: 70.0,
            x2: 95.0,
            y2: 90.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![face],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: PORTRAIT_9_16,
            mode: CropMode::Minimal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1920, 1080, &config).unwrap();

        assert!(
            crop.x + crop.width <= 1920,
            "crop right edge exceeds source"
        );
        assert!(
            crop.y + crop.height <= 1080,
            "crop bottom edge exceeds source"
        );
        assert!(
            focus_inside_crop(&face, &crop, 1920, 1080),
            "corner face center should be inside the crop: crop={crop:?}"
        );

        let frac = overlap_fraction(&face, &crop, 1920, 1080);
        assert!(
            frac >= 0.5,
            "at least 50% of the corner focus region should be visible: overlap={frac:.4}"
        );
    }

    // ── SmartCropInput::compute_crop with focus at (20%, 50%) to (60%, 90%) ──
    // on a 1000x1000 image for 3:4 aspect ratio keeps focus region inside.

    #[test]
    fn compute_crop_focus_region_preserved_3_4_on_square() {
        // Focus region at (20%, 50%) to (60%, 90%) on a 1000x1000 image.
        // 3:4 portrait aspect ratio — requires cropping width to 750px.
        // The focus center X is at 40% (400px), so the crop should shift
        // left to keep the focus region visible.
        let focus = FocusRect {
            x1: 20.0,
            y1: 50.0,
            x2: 60.0,
            y2: 90.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![focus],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: PORTRAIT_3_4,
            mode: CropMode::Minimal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1000, 1000, &config).unwrap();

        assert_eq!(crop.height, 1000, "3:4 on square → full height");
        assert_eq!(crop.width, 750, "3:4 on 1000px height → 750px width");

        assert!(
            focus_inside_crop(&focus, &crop, 1000, 1000),
            "focus center should be inside crop: crop={crop:?}"
        );

        let frac = overlap_fraction(&focus, &crop, 1000, 1000);
        assert!(
            frac >= 0.7,
            "at least 70% of focus region should be visible: overlap={frac:.4}"
        );
    }

    #[test]
    fn compute_crop_focus_region_preserved_landscape_crop() {
        // Focus region at (20%, 50%) to (60%, 90%) on a 1000x1000 image.
        // 16:9 aspect ratio — this requires cropping height.
        // Crop height = 1000 * 9/16 = 562.
        // The focus center Y is at 70% (700px). The crop must shift down to
        // include most of the focus region.
        let focus = FocusRect {
            x1: 20.0,
            y1: 50.0,
            x2: 60.0,
            y2: 90.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![focus],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: LANDSCAPE_16_9,
            mode: CropMode::Minimal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1000, 1000, &config).unwrap();

        assert!(
            crop.x + crop.width <= 1000,
            "crop right edge exceeds source"
        );
        assert!(
            crop.y + crop.height <= 1000,
            "crop bottom edge exceeds source"
        );
        // The crop should have the full width (landscape on a square source).
        assert_eq!(crop.width, 1000, "landscape on square → full width");

        // Focus center should be inside the crop.
        assert!(
            focus_inside_crop(&focus, &crop, 1000, 1000),
            "focus center should be inside the crop: crop={crop:?}, focus center at (400, 700)"
        );

        // At least 70% of the focus region should be visible
        // (the default min_focus_visibility).
        let frac = overlap_fraction(&focus, &crop, 1000, 1000);
        assert!(
            frac >= 0.7,
            "at least 70% of focus region should be visible: overlap={frac:.4}"
        );
    }

    #[test]
    fn compute_crop_focus_region_preserved_portrait_crop() {
        // Focus region at (20%, 50%) to (60%, 90%) on a 1000x1000 image.
        // 9:16 portrait — crops width. Crop width = 1000 * 9/16 = 562.
        // Focus center X is at 40% (400px).
        let focus = FocusRect {
            x1: 20.0,
            y1: 50.0,
            x2: 60.0,
            y2: 90.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![focus],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: PORTRAIT_9_16,
            mode: CropMode::Minimal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1000, 1000, &config).unwrap();

        assert!(
            crop.x + crop.width <= 1000,
            "crop right edge exceeds source"
        );
        assert!(
            crop.y + crop.height <= 1000,
            "crop bottom edge exceeds source"
        );
        assert_eq!(crop.height, 1000, "portrait on square → full height");

        assert!(
            focus_inside_crop(&focus, &crop, 1000, 1000),
            "focus center should be inside the crop: crop={crop:?}"
        );

        let frac = overlap_fraction(&focus, &crop, 1000, 1000);
        assert!(
            frac >= 0.7,
            "at least 70% of focus region should be visible: overlap={frac:.4}"
        );
    }

    // ── Maximal mode zooms into focus area ──

    #[test]
    fn maximal_crop_zooms_into_focus_region() {
        // Focus region at (20%, 50%) to (60%, 90%) on a 1000x1000 image, 1:1 target.
        // Maximal mode should produce a tighter crop than the full image.
        let focus = FocusRect {
            x1: 20.0,
            y1: 50.0,
            x2: 60.0,
            y2: 90.0,
            weight: 0.9,
        };
        let input = SmartCropInput {
            focus_regions: vec![focus],
            heatmap: None,
        };
        let config = CropConfig {
            target_aspect: SQUARE,
            mode: CropMode::Maximal,
            ..CropConfig::default()
        };
        let crop = input.compute_crop(1000, 1000, &config).unwrap();

        assert!(
            crop.x + crop.width <= 1000,
            "crop right edge exceeds source"
        );
        assert!(
            crop.y + crop.height <= 1000,
            "crop bottom edge exceeds source"
        );
        // Maximal mode should zoom in — crop smaller than the full image.
        assert!(
            crop.width < 1000 || crop.height < 1000,
            "maximal mode should zoom in: crop={crop:?}"
        );
        // Crop area should be significantly smaller than source area.
        let crop_area = crop.width as u64 * crop.height as u64;
        let src_area = 1000u64 * 1000;
        assert!(
            crop_area * 100 < src_area * 75,
            "maximal crop area should be < 75% of source: crop_area={crop_area}, src_area={src_area}"
        );
        // Focus center should still be inside.
        assert!(
            focus_inside_crop(&focus, &crop, 1000, 1000),
            "focus center should remain inside maximal crop: crop={crop:?}"
        );
        // Significant overlap with the focus region.
        let frac = overlap_fraction(&focus, &crop, 1000, 1000);
        assert!(
            frac >= 0.7,
            "maximal crop should cover at least 70% of focus region: overlap={frac:.4}"
        );
    }
}
