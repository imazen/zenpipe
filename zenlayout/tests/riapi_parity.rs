//! Parity tests for RIAPI query string → layout computation.
//!
//! Tests that RIAPI query strings produce the same layouts as imageflow_riapi
//! for the common cases. Some rounding differences (±1px) are expected due to
//! zenlayout's snap-rounding vs imageflow's simple rounding.

#![cfg(feature = "riapi")]

use zenlayout::constraint::{CanvasColor, Size};
use zenlayout::riapi;

/// Parse a query, build pipeline, plan it, return (resize_to, canvas, source_crop).
fn query_layout(
    query: &str,
    sw: u32,
    sh: u32,
    exif: Option<u8>,
) -> (Size, Size, Option<zenlayout::Rect>) {
    let result = riapi::parse(query);
    assert!(
        result
            .warnings
            .iter()
            .all(|w| !matches!(w, riapi::ParseWarning::ValueInvalid { .. })),
        "unexpected parse errors for {query:?}: {:?}",
        result.warnings
    );
    let pipeline = result
        .instructions
        .to_pipeline(sw, sh, exif)
        .unwrap_or_else(|e| panic!("pipeline failed for {query:?}: {e:?}"));
    let (ideal, _) = pipeline
        .plan()
        .unwrap_or_else(|e| panic!("plan failed for {query:?}: {e:?}"));
    (
        ideal.layout.resize_to,
        ideal.layout.canvas,
        ideal.layout.source_crop,
    )
}

fn resize(query: &str, sw: u32, sh: u32) -> Size {
    query_layout(query, sw, sh, None).0
}

fn _canvas(query: &str, sw: u32, sh: u32) -> Size {
    query_layout(query, sw, sh, None).1
}

// ============================================================
// Mode × Scale matrix tests
// ============================================================

mod mode_max {
    use super::*;

    #[test]
    fn downscale_only_larger_source() {
        // Within: downscale only. 1000x500 into 800x600 → 800x400
        assert_eq!(
            resize("w=800&h=600&mode=max", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn downscale_only_smaller_source() {
        // Source 200x100 fits in 800x600 → identity
        assert_eq!(
            resize("w=800&h=600&mode=max", 200, 100),
            Size::new(200, 100)
        );
    }

    #[test]
    fn scale_both() {
        // Fit: always scale. 200x100 into 800x600 → 800x400
        assert_eq!(
            resize("w=800&h=600&mode=max&scale=both", 200, 100),
            Size::new(800, 400)
        );
    }

    #[test]
    fn scale_both_larger() {
        assert_eq!(
            resize("w=800&h=600&mode=max&scale=both", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn width_only() {
        assert_eq!(resize("w=500&mode=max", 1000, 500), Size::new(500, 250));
    }

    #[test]
    fn height_only() {
        assert_eq!(resize("h=250&mode=max", 1000, 500), Size::new(500, 250));
    }

    #[test]
    fn no_dimensions_identity() {
        // No w/h → force Max → identity
        assert_eq!(resize("mode=max", 1000, 500), Size::new(1000, 500));
    }
}

mod mode_pad {
    use super::*;

    #[test]
    fn default_mode_is_pad() {
        // When w+h are specified with no mode → default Pad + DownscaleOnly → WithinPad
        let (r, c, _) = query_layout("w=800&h=600", 1000, 500, None);
        assert_eq!(r, Size::new(800, 400));
        assert_eq!(c, Size::new(800, 600));
    }

    #[test]
    fn scale_both() {
        // FitPad: always scale + pad
        let (r, c, _) = query_layout("w=800&h=600&mode=pad&scale=both", 200, 100, None);
        assert_eq!(r, Size::new(800, 400));
        assert_eq!(c, Size::new(800, 600));
    }

    #[test]
    fn downscale_only_small_source() {
        // WithinPad + source fits → identity (no pad, no scale)
        let (r, c, _) = query_layout("w=800&h=600&mode=pad", 200, 100, None);
        assert_eq!(r, Size::new(200, 100));
        assert_eq!(c, Size::new(200, 100));
    }

    #[test]
    fn downscale_only_large_source() {
        let (r, c, _) = query_layout("w=800&h=600&mode=pad", 1000, 500, None);
        assert_eq!(r, Size::new(800, 400));
        assert_eq!(c, Size::new(800, 600));
    }
}

mod mode_crop {
    use super::*;

    #[test]
    fn scale_both_wider_source() {
        // FitCrop: fill target, crop overflow. 1000x500 → 800x600
        // Source wider (2:1) than target (4:3) → crop width
        let (r, _, crop) = query_layout("w=800&h=600&mode=crop&scale=both", 1000, 500, None);
        assert_eq!(r, Size::new(800, 600));
        assert!(crop.is_some());
    }

    #[test]
    fn scale_both_taller_source() {
        // Source taller (1:2) than target (4:3) → crop height
        let (r, _, crop) = query_layout("w=800&h=600&mode=crop&scale=both", 500, 1000, None);
        assert_eq!(r, Size::new(800, 600));
        assert!(crop.is_some());
    }

    #[test]
    fn downscale_both_larger() {
        // WithinCrop: source exceeds both dims → crop to aspect + downscale
        let (r, _, _) = query_layout("w=400&h=300&mode=crop", 1000, 500, None);
        assert_eq!(r, Size::new(400, 300));
    }

    #[test]
    fn downscale_source_fits() {
        // WithinCrop: source fits within target → identity
        let (r, c, _) = query_layout("w=800&h=600&mode=crop", 200, 100, None);
        assert_eq!(r, Size::new(200, 100));
        assert_eq!(c, Size::new(200, 100));
    }

    #[test]
    fn imageflow_ref_768x433_to_100x200() {
        // From imageflow test_crop_and_scale: 768x433, w=100, h=200, mode=crop
        // Expected: crop to 1:2 aspect from 768x433 → crop width to ~217x433
        // then resize to 100x200
        let (r, _, crop) = query_layout("w=100&h=200&mode=crop", 768, 433, None);
        assert_eq!(r, Size::new(100, 200));
        // Crop should remove width (source 768/433=1.77, target 100/200=0.5)
        let c = crop.expect("should crop");
        assert_eq!(c.height, 433, "full height preserved");
        // Width should be ~217 (433 * 100/200 = 216.5)
        assert!(
            c.width >= 216 && c.width <= 217,
            "crop width {}, expected ~217",
            c.width
        );
    }
}

mod mode_stretch {
    use super::*;

    #[test]
    fn scale_both() {
        assert_eq!(
            resize("w=800&h=600&mode=stretch&scale=both", 1000, 500),
            Size::new(800, 600)
        );
    }

    #[test]
    fn downscale_large_source() {
        // Source exceeds target → distort
        assert_eq!(
            resize("w=800&h=600&mode=stretch", 1000, 1000),
            Size::new(800, 600)
        );
    }

    #[test]
    fn downscale_small_source() {
        // Source fits → identity (no distort)
        assert_eq!(
            resize("w=800&h=600&mode=stretch", 200, 100),
            Size::new(200, 100)
        );
    }
}

mod mode_aspect_crop {
    use super::*;

    #[test]
    fn square_from_landscape() {
        let (r, _, crop) = query_layout("w=400&h=400&mode=aspectcrop", 1000, 500, None);
        // Crop to 1:1 from 1000x500 → 500x500
        assert_eq!(r, Size::new(500, 500));
        let c = crop.expect("should crop");
        assert_eq!(c.width, 500);
        assert_eq!(c.height, 500);
    }

    #[test]
    fn square_from_portrait() {
        let (r, _, crop) = query_layout("w=400&h=400&mode=aspectcrop", 500, 1000, None);
        assert_eq!(r, Size::new(500, 500));
        let c = crop.expect("should crop");
        assert_eq!(c.width, 500);
        assert_eq!(c.height, 500);
    }
}

// ============================================================
// Dimension resolution
// ============================================================

mod dimensions {
    use super::*;

    #[test]
    fn maxwidth_caps_w() {
        // w=800, maxwidth=500 → effective w=500
        assert_eq!(
            resize("w=800&maxwidth=500&mode=max", 1000, 500),
            Size::new(500, 250)
        );
    }

    #[test]
    fn maxheight_caps_h() {
        // h=800, maxheight=300 → effective h=300
        assert_eq!(
            resize("h=800&maxheight=300&mode=max", 1000, 500),
            Size::new(600, 300)
        );
    }

    #[test]
    fn maxwidth_alone() {
        // maxwidth=500, no w → effective w=500
        assert_eq!(
            resize("maxwidth=500&mode=max", 1000, 500),
            Size::new(500, 250)
        );
    }

    #[test]
    fn maxheight_alone() {
        assert_eq!(
            resize("maxheight=250&mode=max", 1000, 500),
            Size::new(500, 250)
        );
    }

    #[test]
    fn zoom_doubles_target() {
        assert_eq!(
            resize("w=400&h=300&mode=max&scale=both&zoom=2", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn dpr_suffix_stripped() {
        assert_eq!(
            resize("w=400&h=300&mode=max&scale=both&dpr=2x", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn no_dimensions_no_action() {
        assert_eq!(resize("", 1000, 500), Size::new(1000, 500));
    }

    #[test]
    fn cross_constraint_w_maxheight() {
        // w=800 + maxheight=300. For 1000x500 source:
        // aspect_height_for(800) = 800 * 500/1000 = 400
        // mh = min(300, 400) = 300
        // merge: w=800, h=300
        let r = resize("w=800&maxheight=300&mode=max", 1000, 500);
        assert_eq!(r, Size::new(600, 300));
    }

    #[test]
    fn cross_constraint_h_maxwidth() {
        // h=500 + maxwidth=300. For 1000x500 source:
        // aspect_width_for(500) = 500 * 1000/500 = 1000
        // mw = min(300, 1000) = 300
        // merge: w=300, h=500
        // mode=max: Within, fit 1000x500 into 300x500 → 300x150
        let r = resize("h=500&maxwidth=300&mode=max", 1000, 500);
        assert_eq!(r, Size::new(300, 150));
    }
}

// ============================================================
// Crop parameters
// ============================================================

mod crop_params {
    use super::*;

    #[test]
    fn c_shorthand_percent() {
        // c=10,10,90,90 → crop 10-90% on both axes
        let (r, _, crop) = query_layout(
            "w=400&h=300&mode=max&scale=both&c=10,10,90,90",
            1000,
            500,
            None,
        );
        // Crop: 10-90% of 1000x500 → 800x400 sub-region
        // 800x400 (2:1) fit 400x300 → 400x200
        assert_eq!(r, Size::new(400, 200));
        assert!(crop.is_some());
    }

    #[test]
    fn crop_with_units() {
        // crop=100,100,900,900 with cropxunits=1000, cropyunits=1000
        // → same as crop 10-90%
        let (r, _, _) = query_layout(
            "w=400&h=300&mode=max&scale=both&crop=100,100,900,900&cropxunits=1000&cropyunits=1000",
            1000,
            500,
            None,
        );
        assert_eq!(r, Size::new(400, 200));
    }

    #[test]
    fn crop_pixel_coords() {
        // crop=100,50,500,250 with default units (source pixels for 1000x500)
        let (_, _, crop) = query_layout(
            "w=400&h=300&mode=max&scale=both&crop=100,50,500,250",
            1000,
            500,
            None,
        );
        let c = crop.expect("should have crop");
        // Should crop to roughly 100,50 → 500,250 (400x200 region)
        assert!(c.width >= 395 && c.width <= 405, "width {}", c.width);
        assert!(c.height >= 195 && c.height <= 205, "height {}", c.height);
    }

    #[test]
    fn crop_negative_offset() {
        // crop=0,0,-100,0 → x2 = source_w - 100 = 900
        let (_, _, crop) = query_layout("mode=max&crop=0,0,-100,0", 1000, 500, None);
        let c = crop.expect("should crop");
        assert_eq!(c.y, 0);
        // x2 = -100 + 1000 = 900, width = 900
        assert!(c.width >= 895 && c.width <= 905, "width {}", c.width);
    }
}

// ============================================================
// Orientation
// ============================================================

mod orientation {
    use super::*;

    #[test]
    fn srotate_90() {
        // srotate=90 rotates source: 1000x500 → effective 500x1000
        // Then fit into 800x600 → 300x600
        assert_eq!(
            resize("w=800&h=600&mode=max&srotate=90", 1000, 500),
            Size::new(300, 600)
        );
    }

    #[test]
    fn autorotate_exif_6() {
        // EXIF 6 = Rotate90: 500x1000 → display 1000x500
        let (r, _, _) = query_layout("w=800&h=600&mode=max", 500, 1000, Some(6));
        assert_eq!(r, Size::new(800, 400));
    }

    #[test]
    fn autorotate_false() {
        let (r, _, _) = query_layout("w=800&h=600&mode=max&autorotate=false", 500, 1000, Some(6));
        // EXIF ignored: 500x1000 into 800x600 → 300x600
        assert_eq!(r, Size::new(300, 600));
    }

    #[test]
    fn post_rotate_90_swaps_target() {
        // rotate=90 (post-resize): swaps target w↔h
        // Source 1000x500 + rotate=90 → full orient Rotate90, display 500x1000
        // Target 800x600 → swapped to 600x800
        // 500x1000 fit 600x800 → h-limited: 400x800
        assert_eq!(
            resize("w=800&h=600&mode=max&scale=both&rotate=90", 1000, 500),
            Size::new(400, 800)
        );
    }

    #[test]
    fn post_rotate_180_no_swap() {
        // rotate=180 doesn't swap axes → same dimensions
        assert_eq!(
            resize("w=800&h=600&mode=max&scale=both&rotate=180", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn sflip_no_dimension_change() {
        // Flip never changes dimensions
        assert_eq!(
            resize("w=800&h=600&mode=max&sflip=h", 1000, 500),
            Size::new(800, 400)
        );
    }

    #[test]
    fn flip_no_dimension_change() {
        assert_eq!(
            resize("w=800&h=600&mode=max&flip=both", 1000, 500),
            Size::new(800, 400)
        );
    }
}

// ============================================================
// Gravity / anchor
// ============================================================

mod gravity {
    use super::*;

    #[test]
    fn anchor_topleft_crop() {
        let result = riapi::parse("w=400&h=300&mode=crop&anchor=topleft&scale=both");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        if let Some(crop) = ideal.layout.source_crop {
            assert_eq!(crop.x, 0, "crop should start at left edge");
            assert_eq!(crop.y, 0, "crop should start at top edge");
        }
    }

    #[test]
    fn anchor_bottomright_crop() {
        let result = riapi::parse("w=400&h=300&mode=crop&anchor=bottomright&scale=both");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        let crop = ideal.layout.source_crop.expect("should crop");
        // Crop from bottom-right: x + width = source_w, y + height = source_h
        assert_eq!(crop.x + crop.width, 1000);
        assert_eq!(crop.y + crop.height, 500);
    }

    #[test]
    fn c_gravity_overrides_anchor() {
        let result =
            riapi::parse("w=400&h=300&mode=crop&anchor=topleft&c.gravity=100,100&scale=both");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        let crop = ideal.layout.source_crop.expect("should crop");
        // c.gravity=100,100 → bottom-right
        assert_eq!(crop.x + crop.width, 1000);
        assert_eq!(crop.y + crop.height, 500);
    }

    #[test]
    fn anchor_center_pad_symmetric() {
        let result = riapi::parse("w=800&h=600&mode=pad&scale=both");
        let pipeline = result.instructions.to_pipeline(800, 400, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        // 800x400 into 800x600 → 800x400 content, 200px padding split evenly
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert_eq!(ideal.layout.placement.0, 0); // centered horizontally (exact fit)
        assert_eq!(ideal.layout.placement.1, 100); // 100px from top
    }
}

// ============================================================
// Background color
// ============================================================

mod bgcolor {
    use super::*;

    #[test]
    fn hex_color_applied() {
        let result = riapi::parse("w=800&h=600&bgcolor=ff0000");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
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
    fn named_color_applied() {
        let result = riapi::parse("w=800&h=600&bgcolor=white");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        assert_eq!(
            ideal.layout.canvas_color,
            CanvasColor::Srgb {
                r: 255,
                g: 255,
                b: 255,
                a: 255
            }
        );
    }

    #[test]
    fn default_is_transparent() {
        let result = riapi::parse("w=800&h=600");
        let pipeline = result.instructions.to_pipeline(1000, 500, None).unwrap();
        let (ideal, _) = pipeline.plan().unwrap();
        assert_eq!(ideal.layout.canvas_color, CanvasColor::Transparent);
    }
}

// ============================================================
// Query string parsing
// ============================================================

mod parsing {
    use super::*;

    #[test]
    fn imageflow_ref_rotation_normalization() {
        // srotate=360 → 0, rotate=-90 → 270
        let result = riapi::parse("srotate=360&rotate=-90");
        assert_eq!(result.instructions.srotate, Some(0));
        assert_eq!(result.instructions.rotate, Some(270));
    }

    #[test]
    fn imageflow_ref_rotation_fractional() {
        // srotate=-20.922222 → rounds to 0, rotate=-46.2 → rounds to -90 → 270
        let result = riapi::parse("srotate=-20.922222&rotate=-46.2");
        assert_eq!(result.instructions.srotate, Some(0));
        assert_eq!(result.instructions.rotate, Some(270));
    }

    #[test]
    fn imageflow_ref_flip() {
        let result = riapi::parse("sflip=XY&flip=h");
        assert_eq!(result.instructions.sflip, Some((true, true)));
        assert_eq!(result.instructions.flip, Some((true, false)));
    }

    #[test]
    fn imageflow_ref_flip_none_v() {
        let result = riapi::parse("sflip=None&flip=V");
        assert_eq!(result.instructions.sflip, Some((false, false)));
        assert_eq!(result.instructions.flip, Some((false, true)));
    }

    #[test]
    fn case_insensitive_keys() {
        let result = riapi::parse("Width=20&Height=300&Scale=Canvas");
        assert_eq!(result.instructions.w, Some(20));
        assert_eq!(result.instructions.h, Some(300));
        assert_eq!(
            result.instructions.scale,
            Some(riapi::ScaleMode::UpscaleCanvas)
        );
    }

    #[test]
    fn anchor_bottomleft() {
        let result = riapi::parse("anchor=bottomleft");
        assert_eq!(
            result.instructions.anchor,
            Some((riapi::Anchor1D::Near, riapi::Anchor1D::Far))
        );
    }

    #[test]
    fn anchor_numeric() {
        let result = riapi::parse("anchor=50,25");
        assert_eq!(
            result.instructions.anchor,
            Some((
                riapi::Anchor1D::Percent(50.0),
                riapi::Anchor1D::Percent(25.0)
            ))
        );
    }

    #[test]
    fn c_gravity_values() {
        let result = riapi::parse("c.gravity=89,101");
        assert_eq!(result.instructions.c_gravity, Some([89.0, 101.0]));
    }

    #[test]
    fn legacy_shortcuts() {
        // stretch=fill → mode=stretch, crop=auto → mode=crop
        let result = riapi::parse("stretch=fill");
        assert_eq!(result.instructions.mode, Some(riapi::FitMode::Stretch));

        let result = riapi::parse("crop=auto");
        assert_eq!(result.instructions.mode, Some(riapi::FitMode::Crop));
    }

    #[test]
    fn mode_overrides_shortcuts() {
        // Explicit mode takes priority over stretch=fill
        let result = riapi::parse("mode=max&stretch=fill");
        assert_eq!(result.instructions.mode, Some(riapi::FitMode::Max));
    }

    #[test]
    fn bgcolor_css_names() {
        let colors = [
            ("red", 255, 0, 0),
            ("darkseagreen", 143, 188, 139),
            ("lightslategray", 119, 136, 153),
        ];
        for (name, r, g, b) in colors {
            let result = riapi::parse(&format!("bgcolor={name}"));
            assert_eq!(
                result.instructions.bgcolor,
                Some(CanvasColor::Srgb { r, g, b, a: 255 }),
                "color {name}"
            );
        }
    }

    #[test]
    fn bgcolor_hex_with_alpha() {
        let result = riapi::parse("bgcolor=77889953");
        assert_eq!(
            result.instructions.bgcolor,
            Some(CanvasColor::Srgb {
                r: 0x77,
                g: 0x88,
                b: 0x99,
                a: 0x53
            })
        );
    }

    #[test]
    fn extras_preserved() {
        let result = riapi::parse("w=800&format=webp&quality=80&f.sharpen=10");
        assert_eq!(
            result
                .instructions
                .extras()
                .get("format")
                .map(String::as_str),
            Some("webp")
        );
        assert_eq!(
            result
                .instructions
                .extras()
                .get("quality")
                .map(String::as_str),
            Some("80")
        );
        assert_eq!(
            result
                .instructions
                .extras()
                .get("f.sharpen")
                .map(String::as_str),
            Some("10")
        );
    }
}

// ============================================================
// imageflow_riapi reference dimensions (from layout tests)
// ============================================================

mod imageflow_reference {
    use super::*;

    #[test]
    fn max_mode_5104x3380_to_2560x1696() {
        // From imageflow test_scale: 5104x3380, w=2560, h=1696, mode=max
        // Expected: resize to 2560x1695 (aspect preserved, no upscale)
        let r = resize("w=2560&h=1696&mode=max", 5104, 3380);
        assert_eq!(r.width, 2560);
        // Height: 5104x3380 fit 2560x1696 → width-limited: 2560 × (3380/5104) = 1695.14 → 1695
        assert!(
            r.height >= 1694 && r.height <= 1696,
            "height {}, expected ~1695",
            r.height
        );
    }

    #[test]
    fn crop_mode_768x433_to_100x200() {
        // From imageflow test_crop_and_scale
        let (r, _, crop) = query_layout("w=100&h=200&mode=crop", 768, 433, None);
        assert_eq!(r, Size::new(100, 200));
        let c = crop.expect("should crop");
        // Imageflow: crop [275, 0, 492, 433] → width=217, full height
        assert_eq!(c.y, 0);
        assert_eq!(c.height, 433);
        assert!(
            c.width >= 216 && c.width <= 218,
            "crop width {}, expected ~217",
            c.width
        );
    }

    #[test]
    fn default_pad_1000x500_to_800x600() {
        // Default mode (Pad) + default scale (Down) → WithinPad
        let (r, c, _) = query_layout("w=800&h=600", 1000, 500, None);
        assert_eq!(r, Size::new(800, 400));
        assert_eq!(c, Size::new(800, 600));
    }

    #[test]
    fn crop_mode_with_both_1000x500_to_800x600() {
        let (r, _, _) = query_layout("w=800&h=600&mode=crop&scale=both", 1000, 500, None);
        assert_eq!(r, Size::new(800, 600));
    }
}

// ============================================================
// Edge cases
// ============================================================

mod edge_cases {
    use super::*;

    #[test]
    fn very_large_zoom() {
        // Zoom is clamped to 80000
        let r = resize("w=1&h=1&zoom=999999&mode=max&scale=both", 100, 100);
        // w*80000 = 80000, h*80000 = 80000 → 80000x80000
        assert!(r.width > 0 && r.height > 0);
    }

    #[test]
    fn very_small_zoom() {
        let r = resize("w=10000&h=10000&zoom=0.0001&mode=max&scale=both", 100, 100);
        // Clamped to 0.00008. 10000*0.00008 = 0.8 → rounds to 1
        assert!(r.width >= 1 && r.height >= 1);
    }

    #[test]
    fn square_source_square_target() {
        assert_eq!(
            resize("w=100&h=100&mode=max&scale=both", 100, 100),
            Size::new(100, 100)
        );
        assert_eq!(
            resize("w=100&h=100&mode=crop&scale=both", 100, 100),
            Size::new(100, 100)
        );
    }

    #[test]
    fn extreme_aspect_ratio() {
        // Very wide source
        let r = resize("w=100&h=100&mode=crop&scale=both", 10000, 1);
        assert_eq!(r, Size::new(100, 100));

        // Very tall source
        let r = resize("w=100&h=100&mode=crop&scale=both", 1, 10000);
        assert_eq!(r, Size::new(100, 100));
    }

    #[test]
    fn empty_query_string() {
        let result = riapi::parse("");
        assert!(result.warnings.is_empty());
        let r = resize("", 500, 500);
        assert_eq!(r, Size::new(500, 500));
    }

    #[test]
    fn only_unknown_keys() {
        let result = riapi::parse("foo=bar&baz=qux");
        assert_eq!(result.warnings.len(), 2);
        assert!(
            result
                .warnings
                .iter()
                .all(|w| matches!(w, riapi::ParseWarning::KeyNotRecognized { .. }))
        );
    }
}
