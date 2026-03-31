//! Regression tests for RIAPI c.gravity and focus rectangle parameters.
//!
//! Covers GitHub issues related to:
//! - `c.gravity=x,y` focal point parsing and crop positioning
//! - `focus=x1,y1,x2,y2` focus rectangle parameter (RIAPI → smart_crop)

// ════════════════════════════════════════════════════════════════════
// Issue 1: c.gravity=x,y focal point parameter
// ════════════════════════════════════════════════════════════════════

#[cfg(feature = "riapi")]
mod c_gravity {
    use zenlayout::constraint::Size;
    use zenlayout::riapi;

    /// Helper: parse query, build pipeline, plan it, return the IdealLayout.
    fn plan_layout(query: &str, sw: u32, sh: u32) -> zenlayout::plan::IdealLayout {
        let result = riapi::parse(query);
        for w in &result.warnings {
            if matches!(
                w,
                riapi::ParseWarning::ValueInvalid { .. }
                    | riapi::ParseWarning::KeyNotRecognized { .. }
            ) {
                panic!("unexpected parse warning for {query:?}: {w:?}");
            }
        }
        let pipeline = result
            .instructions
            .to_pipeline(sw, sh, None)
            .unwrap_or_else(|e| panic!("pipeline failed for {query:?}: {e:?}"));
        let (ideal, _) = pipeline
            .plan()
            .unwrap_or_else(|e| panic!("plan failed for {query:?}: {e:?}"));
        ideal
    }

    // ── Parsing: c.gravity=30,70 produces Gravity::Percentage(0.3, 0.7) ──

    #[test]
    fn c_gravity_30_70_parses_to_percentage() {
        // Parse c.gravity=30,70 and verify the raw parsed values.
        let result = riapi::parse("c.gravity=30,70");
        assert_eq!(
            result.instructions.c_gravity,
            Some([30.0, 70.0]),
            "raw c_gravity should be [30.0, 70.0]"
        );
    }

    #[test]
    fn c_gravity_30_70_resolves_to_percentage_0_3_0_7() {
        // Verify the full pipeline: c.gravity=30,70 should produce a Constraint
        // with Gravity::Percentage(0.3, 0.7) after resolve_gravity divides by 100.
        //
        // We can verify this indirectly: in crop mode, the gravity affects where
        // the crop rectangle is placed. A 1000x1000 source cropped to 200x200
        // with c.gravity=30,70 should place the crop offset at 30% horizontally
        // and 70% vertically of the available space.
        let ideal = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=30,70",
            1000,
            500,
        );
        assert_eq!(ideal.layout.resize_to, Size::new(200, 200));

        // Source 1000x500 cropped to 2:1→1:1 means cropping width.
        // New width = 500 (to match 1:1 at full height).
        // Horizontal slack = 1000 - 500 = 500.
        // At gravity x=0.3: offset = round(500 * 0.3) = 150.
        let crop = ideal
            .layout
            .source_crop
            .expect("crop mode should produce source_crop");
        assert_eq!(crop.height, 500, "full height should be preserved");
        assert_eq!(crop.width, 500, "crop width should be 500 for 1:1 aspect");
        assert_eq!(crop.x, 150, "x offset should be 30% of 500px slack = 150");
        assert_eq!(crop.y, 0, "y offset should be 0 (no vertical cropping)");
    }

    // ── Crop positioning: gravity=0,0 crops from top-left ──

    #[test]
    fn c_gravity_0_0_crops_from_top_left() {
        // 1000x1000 source cropped to 500x500 (1:1 target at scale=both, mode=crop)
        // Source is already 1:1, so actually let's use a non-square source.
        // 1000x800 → 200x200 crop. Aspect: source 5:4, target 1:1.
        // Source wider → crop width. new_w = 800 * 1/1 = 800. slack = 1000 - 800 = 200.
        // gravity=0,0 → x = 0% of 200 = 0
        let ideal = plan_layout("w=200&h=200&mode=crop&scale=both&c.gravity=0,0", 1000, 800);
        let crop = ideal.layout.source_crop.expect("should have crop");
        assert_eq!(crop.x, 0, "gravity 0,0 should crop from left edge");
        assert_eq!(crop.y, 0, "gravity 0,0 should crop from top edge");
    }

    // ── Crop positioning: gravity=100,100 crops from bottom-right ──

    #[test]
    fn c_gravity_100_100_crops_from_bottom_right() {
        // 1000x800 → 200x200 target, mode=crop, scale=both.
        // Source wider (5:4) than target (1:1) → crop width.
        // new_w = 800 (match height for 1:1). slack = 200.
        // gravity=100,100 → x = 100% of 200 = 200
        let ideal = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=100,100",
            1000,
            800,
        );
        let crop = ideal.layout.source_crop.expect("should have crop");
        assert_eq!(
            crop.x + crop.width,
            1000,
            "gravity 100,100 should push crop to right edge"
        );
        assert_eq!(
            crop.y + crop.height,
            800,
            "gravity 100,100 should push crop to bottom edge"
        );
    }

    // ── c.gravity with FitCrop (mode=crop) interaction ──

    #[test]
    fn c_gravity_with_mode_crop_positions_correctly() {
        // 1000x500 source, target 400x400 (1:1), mode=crop, scale=both.
        // Source wider (2:1) vs target (1:1) → crop width.
        // new_w = 500 (height for 1:1). slack = 1000 - 500 = 500.
        //
        // c.gravity=50,50 should center the crop → x = 250.
        let ideal_center = plan_layout(
            "w=400&h=400&mode=crop&scale=both&c.gravity=50,50",
            1000,
            500,
        );
        let crop_center = ideal_center.layout.source_crop.expect("should crop");
        assert_eq!(crop_center.x, 250, "50% gravity → x=250");

        // c.gravity=0,50 → x=0 (left edge)
        let ideal_left = plan_layout("w=400&h=400&mode=crop&scale=both&c.gravity=0,50", 1000, 500);
        let crop_left = ideal_left.layout.source_crop.expect("should crop");
        assert_eq!(crop_left.x, 0, "0% horizontal gravity → left edge");

        // c.gravity=100,50 → x=500 (right edge)
        let ideal_right = plan_layout(
            "w=400&h=400&mode=crop&scale=both&c.gravity=100,50",
            1000,
            500,
        );
        let crop_right = ideal_right.layout.source_crop.expect("should crop");
        assert_eq!(crop_right.x, 500, "100% horizontal gravity → right edge");
    }

    // ── c.gravity affects pad placement ──

    #[test]
    fn c_gravity_affects_pad_placement() {
        // 800x400 source into 800x600 pad target → 200px vertical padding.
        // c.gravity=50,0 → top-aligned: placement y=0
        let ideal_top = plan_layout("w=800&h=600&mode=pad&scale=both&c.gravity=50,0", 800, 400);
        assert_eq!(ideal_top.layout.placement.1, 0, "gravity y=0 → top-aligned");

        // c.gravity=50,100 → bottom-aligned: placement y=200
        let ideal_bottom =
            plan_layout("w=800&h=600&mode=pad&scale=both&c.gravity=50,100", 800, 400);
        assert_eq!(
            ideal_bottom.layout.placement.1, 200,
            "gravity y=100 → bottom-aligned"
        );

        // c.gravity=50,50 → centered: placement y=100
        let ideal_center = plan_layout("w=800&h=600&mode=pad&scale=both&c.gravity=50,50", 800, 400);
        assert_eq!(
            ideal_center.layout.placement.1, 100,
            "gravity y=50 → centered"
        );
    }

    // ── c.gravity 50,50 matches default center gravity ──

    #[test]
    fn c_gravity_50_50_matches_default_center() {
        let ideal_default = plan_layout("w=400&h=400&mode=crop&scale=both", 1000, 500);
        let ideal_50_50 = plan_layout(
            "w=400&h=400&mode=crop&scale=both&c.gravity=50,50",
            1000,
            500,
        );
        assert_eq!(
            ideal_default.layout.source_crop, ideal_50_50.layout.source_crop,
            "c.gravity=50,50 should match default center gravity for crop"
        );
    }

    // ── c.gravity values are clamped to 0-100 ──

    #[test]
    fn c_gravity_clamped_at_boundaries() {
        // c.gravity=-50,-50 should behave like 0,0 (clamped)
        let ideal_neg = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=-50,-50",
            1000,
            800,
        );
        let ideal_zero = plan_layout("w=200&h=200&mode=crop&scale=both&c.gravity=0,0", 1000, 800);
        assert_eq!(
            ideal_neg.layout.source_crop, ideal_zero.layout.source_crop,
            "negative gravity should be clamped to 0"
        );

        // c.gravity=200,200 should behave like 100,100 (clamped)
        let ideal_over = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=200,200",
            1000,
            800,
        );
        let ideal_100 = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=100,100",
            1000,
            800,
        );
        assert_eq!(
            ideal_over.layout.source_crop, ideal_100.layout.source_crop,
            "gravity >100 should be clamped to 100"
        );
    }

    // ── c.gravity with taller source (vertical crop) ──

    #[test]
    fn c_gravity_vertical_crop_on_tall_source() {
        // 500x1000 source, target 200x200 (1:1), mode=crop.
        // Source taller (1:2) vs target (1:1) → crop height.
        // new_h = 500 (match width for 1:1). slack = 1000 - 500 = 500.
        //
        // c.gravity=50,25 → y = 25% of 500 = 125
        let ideal = plan_layout(
            "w=200&h=200&mode=crop&scale=both&c.gravity=50,25",
            500,
            1000,
        );
        let crop = ideal.layout.source_crop.expect("should have crop");
        assert_eq!(crop.width, 500, "full width preserved");
        assert_eq!(crop.height, 500, "crop height = 500 for 1:1 aspect");
        assert_eq!(crop.x, 0, "no horizontal cropping");
        assert_eq!(crop.y, 125, "y offset = 25% of 500px slack = 125");
    }
}

// ════════════════════════════════════════════════════════════════════
// Issue 2: focus=x1,y1,x2,y2 focus rectangle parameter
// ════════════════════════════════════════════════════════════════════

#[cfg(feature = "riapi")]
mod c_focus_parsing {
    use zenlayout::riapi;
    use zenlayout::riapi::CFocus;

    /// The bare `focus` key (without `c.` prefix) is not recognized.
    #[test]
    fn bare_focus_key_unrecognized() {
        let result = riapi::parse("w=200&h=200&mode=crop&focus=20,50,60,90");
        let has_warning = result.warnings.iter().any(|w| {
            matches!(
                w,
                riapi::ParseWarning::KeyNotRecognized { key, .. } if key == "focus"
            )
        });
        assert!(
            has_warning,
            "bare `focus` key should produce KeyNotRecognized"
        );
    }

    /// `c.focus=50,30` parses as a focal point.
    #[test]
    fn c_focus_point_two_values() {
        let result = riapi::parse("w=200&h=200&mode=crop&c.focus=50,30");
        assert!(
            result.warnings.is_empty(),
            "should parse without warnings: {:?}",
            result.warnings
        );
        assert_eq!(
            result.instructions.c_focus,
            Some(CFocus::Point([50.0, 30.0]))
        );
    }

    /// `c.focus=20,30,80,90` parses as a single focus rectangle.
    #[test]
    fn c_focus_single_rect() {
        let result = riapi::parse("c.focus=20,30,80,90");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(
            result.instructions.c_focus,
            Some(CFocus::Rects(vec![[20.0, 30.0, 80.0, 90.0]]))
        );
    }

    /// `c.focus=20,30,80,90,10,10,40,40` parses as two focus rectangles.
    #[test]
    fn c_focus_multiple_rects() {
        let result = riapi::parse("c.focus=20,30,80,90,10,10,40,40");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(
            result.instructions.c_focus,
            Some(CFocus::Rects(vec![
                [20.0, 30.0, 80.0, 90.0],
                [10.0, 10.0, 40.0, 40.0],
            ]))
        );
    }

    /// `c.focus=faces` parses as the Faces keyword.
    #[test]
    fn c_focus_faces_keyword() {
        let result = riapi::parse("c.focus=faces");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_focus, Some(CFocus::Faces));
    }

    /// Case-insensitive keyword matching.
    #[test]
    fn c_focus_faces_keyword_case_insensitive() {
        let result = riapi::parse("c.focus=Faces");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_focus, Some(CFocus::Faces));
    }

    /// `c.focus=auto` parses as the Auto keyword.
    #[test]
    fn c_focus_auto_keyword() {
        let result = riapi::parse("c.focus=auto");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_focus, Some(CFocus::Auto));
    }

    /// Invalid value produces a warning, does not panic.
    #[test]
    fn c_focus_invalid_warns_no_crash() {
        let result = riapi::parse("c.focus=abc");
        assert!(result.instructions.c_focus.is_none());
        let has_invalid = result
            .warnings
            .iter()
            .any(|w| matches!(w, riapi::ParseWarning::ValueInvalid { key: "c.focus", .. }));
        assert!(has_invalid, "invalid c.focus should produce ValueInvalid");
    }

    /// Three values (not 2 or multiple of 4) produces a warning.
    #[test]
    fn c_focus_three_values_warns() {
        let result = riapi::parse("c.focus=1,2,3");
        assert!(result.instructions.c_focus.is_none());
        let has_invalid = result
            .warnings
            .iter()
            .any(|w| matches!(w, riapi::ParseWarning::ValueInvalid { key: "c.focus", .. }));
        assert!(has_invalid, "3-value c.focus should produce ValueInvalid");
    }

    /// `c.zoom=true` parses correctly.
    #[test]
    fn c_zoom_true() {
        let result = riapi::parse("c.zoom=true");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_zoom, Some(true));
    }

    /// `c.zoom=false` parses correctly.
    #[test]
    fn c_zoom_false() {
        let result = riapi::parse("c.zoom=false");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_zoom, Some(false));
    }

    /// `c.zoom=1` parses as true.
    #[test]
    fn c_zoom_one_is_true() {
        let result = riapi::parse("c.zoom=1");
        assert_eq!(result.instructions.c_zoom, Some(true));
    }

    /// `c.zoom=0` parses as false.
    #[test]
    fn c_zoom_zero_is_false() {
        let result = riapi::parse("c.zoom=0");
        assert_eq!(result.instructions.c_zoom, Some(false));
    }

    /// `c.finalmode=pad` stores the string value.
    #[test]
    fn c_finalmode_pad() {
        let result = riapi::parse("c.finalmode=pad");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.instructions.c_finalmode.as_deref(), Some("pad"));
    }

    /// `c.finalmode=crop` stores the string value.
    #[test]
    fn c_finalmode_crop() {
        let result = riapi::parse("c.finalmode=crop");
        assert_eq!(result.instructions.c_finalmode.as_deref(), Some("crop"));
    }

    /// Duplicate c.focus warns.
    #[test]
    fn c_focus_duplicate_warns() {
        let result = riapi::parse("c.focus=50,50&c.focus=faces");
        let has_dup = result
            .warnings
            .iter()
            .any(|w| matches!(w, riapi::ParseWarning::DuplicateKey { .. }));
        assert!(has_dup, "duplicate c.focus should warn");
        // Last value wins
        assert_eq!(result.instructions.c_focus, Some(CFocus::Faces));
    }

    /// focus_needs_detection returns true for Faces/Auto, false otherwise.
    #[test]
    fn focus_needs_detection() {
        let result = riapi::parse("c.focus=faces");
        assert!(result.instructions.focus_needs_detection());

        let result = riapi::parse("c.focus=auto");
        assert!(result.instructions.focus_needs_detection());

        let result = riapi::parse("c.focus=50,50");
        assert!(!result.instructions.focus_needs_detection());

        let result = riapi::parse("c.focus=10,20,80,90");
        assert!(!result.instructions.focus_needs_detection());

        let result = riapi::parse("w=100");
        assert!(!result.instructions.focus_needs_detection());
    }
}

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
