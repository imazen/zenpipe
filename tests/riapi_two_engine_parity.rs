//! W9: two-engine RIAPI geometry parity.
//!
//! Runs the same querystring through BOTH engines — the legacy full-IR4
//! parser (`expand_legacy`, imageflow_riapi's own layout engine) and the
//! zen-native registry path (`expand_zen`) — compiles each node list into a
//! pipeline against the same source dimensions, and asserts the OUTPUT
//! GEOMETRY matches within ±1 px (integer-rounding slack between the two
//! layout engines).
//!
//! This is the verification layer for the 2026-07-11 divergence fixes
//! (IMAGEFLOW-PARITY.md §4): a case that fails here is a real semantic
//! divergence between what ImageResizer/imageflow would produce and what
//! zenpipe's native path produces.

#![cfg(all(feature = "imageflow-compat", feature = "nodes-all"))]

use zenpipe::imageflow_compat::riapi::{expand_legacy, expand_zen};

const SRC_W: u32 = 400;
const SRC_H: u32 = 300;

/// Compile a node list against a dummy source and return output dimensions.
fn output_dims(nodes: &[Box<dyn zennode::NodeInstance>], w: u32, h: u32) -> (u32, u32) {
    let data = vec![0u8; (w * h * 4) as usize];
    let source = Box::new(zenpipe::sources::MaterializedSource::from_data(
        data,
        w,
        h,
        zenpipe::format::RGBA8_SRGB,
    ));
    let converters = zenpipe::imageflow_compat::converter::imageflow_converters();
    let converters: &[&dyn zenpipe::bridge::NodeConverter] = &converters;
    let result = zenpipe::bridge::build_pipeline(source, nodes, converters)
        .unwrap_or_else(|e| panic!("pipeline compile failed: {e}"));
    (result.source.width(), result.source.height())
}

fn legacy_dims(qs: &str) -> (u32, u32) {
    let expanded = expand_legacy(qs, SRC_W as i32, SRC_H as i32, None, false, 1, Some(1))
        .unwrap_or_else(|e| panic!("expand_legacy({qs}) failed: {e:?}"));
    output_dims(&expanded.nodes, SRC_W, SRC_H)
}

fn zen_dims(qs: &str) -> (u32, u32) {
    let expanded = expand_zen(qs, SRC_W, SRC_H, None)
        .unwrap_or_else(|e| panic!("expand_zen({qs}) failed: {e:?}"));
    output_dims(&expanded.nodes, SRC_W, SRC_H)
}

fn assert_parity(qs: &str) {
    let (lw, lh) = legacy_dims(qs);
    let (zw, zh) = zen_dims(qs);
    let dw = (lw as i64 - zw as i64).abs();
    let dh = (lh as i64 - zh as i64).abs();
    assert!(
        dw <= 1 && dh <= 1,
        "geometry divergence for '{qs}': legacy {lw}x{lh} vs zen {zw}x{zh} (source {SRC_W}x{SRC_H})"
    );
}

macro_rules! parity_case {
    ($name:ident, $qs:expr) => {
        #[test]
        fn $name() {
            assert_parity($qs);
        }
    };
}

// ── sizing / fit modes / scale ──
parity_case!(w_only, "w=100");
parity_case!(h_only, "h=100");
parity_case!(w_h_default_mode, "w=100&h=100");
parity_case!(mode_max, "w=100&h=100&mode=max");
parity_case!(mode_pad, "w=100&h=100&mode=pad");
parity_case!(mode_crop, "w=100&h=100&mode=crop");
parity_case!(mode_stretch, "w=100&h=100&mode=stretch");
parity_case!(mode_max_scale_both, "w=800&h=800&mode=max&scale=both");
parity_case!(mode_pad_scale_both, "w=800&h=800&mode=pad&scale=both");
parity_case!(mode_crop_scale_both, "w=800&h=800&mode=crop&scale=both");
parity_case!(
    mode_stretch_scale_both,
    "w=800&h=800&mode=stretch&scale=both"
);
parity_case!(upscale_denied_by_default, "w=800&h=800");
parity_case!(
    scale_up_smaller_source_upscales,
    "w=800&h=600&mode=max&scale=up"
);
parity_case!(scale_up_larger_source_noop, "w=100&h=75&mode=max&scale=up");
parity_case!(scale_canvas_pads, "w=800&h=800&mode=max&scale=canvas");
parity_case!(maxwidth_maxheight, "maxwidth=100&maxheight=100");
parity_case!(legacy_stretch_fill, "w=100&h=100&stretch=fill");
parity_case!(legacy_crop_auto, "w=100&h=100&crop=auto");
parity_case!(aspectcrop, "w=100&h=50&mode=aspectcrop");
parity_case!(zoom_multiplies, "w=100&h=100&zoom=2");
parity_case!(dpr_alias, "w=100&h=100&dpr=1.5");

// ── manual crop ──
parity_case!(crop_pixels_default_units, "crop=100,50,300,250");
parity_case!(crop_then_resize, "crop=100,50,300,250&w=50&h=50&mode=max");
parity_case!(
    crop_percent_units,
    "crop=10,10,90,90&cropxunits=100&cropyunits=100"
);
parity_case!(crop_c_shorthand, "c=25,25,75,75");
parity_case!(crop_negative_coords, "crop=-100,-100,0,0");
parity_case!(crop_inverted_resets, "crop=300,200,100,50");

// ── rotate / flip ──
parity_case!(rotate_90_swaps, "rotate=90");
parity_case!(rotate_270_swaps, "rotate=270");
parity_case!(rotate_180_keeps, "rotate=180");
parity_case!(srotate_90_swaps, "srotate=90");
parity_case!(flip_h_keeps_dims, "flip=h");
parity_case!(sflip_v_keeps_dims, "sflip=v");
parity_case!(srotate_crop_resize, "srotate=90&crop=10,10,200,200&w=50");
parity_case!(rotate_after_pad, "w=100&h=50&mode=pad&rotate=90");

// ── combinations ──
parity_case!(
    anchor_does_not_change_dims,
    "w=100&h=100&mode=crop&anchor=topleft"
);
parity_case!(
    bgcolor_does_not_change_dims,
    "w=100&h=100&mode=pad&bgcolor=red"
);
parity_case!(sharpen_does_not_change_dims, "w=100&f.sharpen=20");

// ── maxwidth / maxheight bounding ──
parity_case!(maxwidth_only, "maxwidth=100");
parity_case!(w_with_same_axis_maxwidth, "w=200&maxwidth=100&h=150");
parity_case!(w_with_cross_axis_maxheight, "w=200&maxheight=50");
parity_case!(h_with_cross_axis_maxwidth, "h=200&maxwidth=100");

// ── larger_than (zenlayout mode, now accepted by the bridge) ──
#[test]
fn larger_than_matches_imageflow_max_upscale_only() {
    // imageflow defines larger_than as Max + UpscaleOnly
    // (imageflow_riapi/src/ir4/layout.rs:307-310): sources that fit inside
    // the box on both axes scale to the INNER fit; larger sources pass
    // through. 400×300 into 800×800 → 800×600.
    let expanded =
        expand_zen("w=800&h=800&mode=larger_than", SRC_W, SRC_H, None).expect("expand larger_than");
    assert_eq!(output_dims(&expanded.nodes, SRC_W, SRC_H), (800, 600));

    // Larger source passes through unchanged.
    let expanded =
        expand_zen("w=100&h=100&mode=larger_than", SRC_W, SRC_H, None).expect("expand larger_than");
    assert_eq!(output_dims(&expanded.nodes, SRC_W, SRC_H), (SRC_W, SRC_H));
}
