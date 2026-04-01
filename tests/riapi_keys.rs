//! RIAPI querystring key parity tests.
//!
//! Verifies that the zen registry handles the same querystring keys as
//! imageflow_riapi, producing the correct node instances with the correct
//! parameter values.

#![cfg(feature = "zennode")]

use zennode::NodeInstance;

fn registry() -> zennode::NodeRegistry {
    zenpipe::full_registry()
}

fn parse(qs: &str) -> zennode::registry::KvResult {
    registry().from_querystring(qs)
}

fn find_node<'a>(
    instances: &'a [Box<dyn NodeInstance>],
    schema_id: &str,
) -> Option<&'a dyn NodeInstance> {
    instances
        .iter()
        .find(|n| n.schema().id == schema_id)
        .map(|n| n.as_ref())
}

fn get_str(node: &dyn NodeInstance, param: &str) -> Option<String> {
    node.get_param(param)?.as_str().map(|s| s.to_string())
}

fn get_f32(node: &dyn NodeInstance, param: &str) -> Option<f32> {
    node.get_param(param)?.as_f32()
}

fn get_u32(node: &dyn NodeInstance, param: &str) -> Option<u32> {
    node.get_param(param)?.as_u32()
}

fn get_bool(node: &dyn NodeInstance, param: &str) -> Option<bool> {
    node.get_param(param)?.as_bool()
}

// ═══════════════════════════════════════════════════════════════════════
//  SIZING KEYS
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn sizing_w_h() {
    let r = parse("w=800&h=600");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_u32(c, "w"), Some(800));
    assert_eq!(get_u32(c, "h"), Some(600));
}

#[test]
fn sizing_width_height_aliases() {
    let r = parse("width=1024&height=768");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_u32(c, "w"), Some(1024));
    assert_eq!(get_u32(c, "h"), Some(768));
}

#[test]
fn sizing_maxwidth_maxheight() {
    let r = parse("maxwidth=500&maxheight=400");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_u32(c, "w"), Some(500));
    assert_eq!(get_u32(c, "h"), Some(400));
}

// ═══════════════════════════════════════════════════════════════════════
//  MODE & SCALE
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn mode_crop() {
    let r = parse("w=800&h=600&mode=crop");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "mode").as_deref(), Some("crop"));
}

#[test]
fn mode_pad() {
    let r = parse("w=800&h=600&mode=pad");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "mode").as_deref(), Some("pad"));
}

#[test]
fn scale_key() {
    let r = parse("w=800&scale=down");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "scale").as_deref(), Some("down"));
}

// ═══════════════════════════════════════════════════════════════════════
//  ZOOM / DPR
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn zoom_key() {
    let r = parse("w=400&zoom=2");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_f32(c, "zoom"), Some(2.0));
}

#[test]
fn dpr_key_on_constrain() {
    // dpr consumed by both Constrain (zoom) and QualityIntent (dpr)
    let r = parse("w=400&dpr=1.5");
    let c = find_node(&r.instances, "zenresize.constrain");
    let q = find_node(&r.instances, "zencodecs.quality_intent");
    // At least one should have consumed it
    let constrain_has = c.and_then(|c| get_f32(c, "zoom")).is_some();
    let qi_has = q.and_then(|q| get_f32(q, "dpr")).is_some();
    assert!(
        constrain_has || qi_has,
        "dpr should be consumed by Constrain or QualityIntent"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  ANCHOR / GRAVITY
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn anchor_key() {
    let r = parse("w=800&h=600&mode=crop&anchor=topleft");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "gravity").as_deref(), Some("topleft"));
}

// ═══════════════════════════════════════════════════════════════════════
//  AUTOROTATE
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn srotate_90() {
    // srotate takes an integer (90, 180, 270) — this works with the i32 field.
    let r = parse("srotate=6");
    let o = find_node(&r.instances, "zenlayout.orient").expect("Orient node");
    assert!(get_u32(o, "orientation").is_some() || get_str(o, "orientation").is_some());
}

// ═══════════════════════════════════════════════════════════════════════
//  FLIP (RIAPI adapter)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn flip_horizontal() {
    let r = parse("flip=h");
    assert!(
        find_node(&r.instances, "zenlayout.flip_h").is_some(),
        "flip=h → FlipH"
    );
}

#[test]
fn flip_vertical() {
    let r = parse("flip=v");
    assert!(
        find_node(&r.instances, "zenlayout.flip_v").is_some(),
        "flip=v → FlipV"
    );
}

#[test]
fn flip_both() {
    let r = parse("flip=both");
    assert!(
        find_node(&r.instances, "zenlayout.rotate_180").is_some(),
        "flip=both → Rotate180"
    );
}

#[test]
fn sflip_alias() {
    let r = parse("sflip=x");
    assert!(
        find_node(&r.instances, "zenlayout.flip_h").is_some(),
        "sflip=x → FlipH"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  ROTATE (RIAPI adapter)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn rotate_90() {
    let r = parse("rotate=90");
    assert!(find_node(&r.instances, "zenlayout.rotate_90").is_some());
}

#[test]
fn rotate_180() {
    let r = parse("rotate=180");
    assert!(find_node(&r.instances, "zenlayout.rotate_180").is_some());
}

#[test]
fn rotate_270() {
    let r = parse("rotate=270");
    assert!(find_node(&r.instances, "zenlayout.rotate_270").is_some());
}

#[test]
fn rotate_0_no_node() {
    let r = parse("rotate=0");
    assert!(find_node(&r.instances, "zenlayout.rotate_90").is_none());
    assert!(find_node(&r.instances, "zenlayout.rotate_180").is_none());
    assert!(find_node(&r.instances, "zenlayout.rotate_270").is_none());
}

// ═══════════════════════════════════════════════════════════════════════
//  AUTOROTATE (RIAPI adapter)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn autorotate_true() {
    let r = parse("autorotate=true");
    assert!(
        find_node(&r.instances, "zenlayout.orient").is_some(),
        "autorotate=true → Orient"
    );
}

#[test]
fn autorotate_false_no_node() {
    let r = parse("autorotate=false");
    // autorotate=false should not produce an orient node from the adapter
    // (the derive-based Orient node may still match srotate, but not autorotate)
    let from_adapter: Vec<_> = r
        .instances
        .iter()
        .filter(|n| n.schema().id == "zenpipe.riapi.autorotate")
        .collect();
    assert!(from_adapter.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
//  FRAME SELECT (RIAPI adapter)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn frame_select() {
    let r = parse("frame=3");
    let f = find_node(&r.instances, "zenpipe.riapi.frame").expect("frame=3 → FrameSelect");
    assert_eq!(get_u32(f, "frame"), Some(3));
}

#[test]
fn page_alias_for_frame() {
    let r = parse("page=0");
    let f = find_node(&r.instances, "zenpipe.riapi.frame").expect("page=0 → FrameSelect");
    assert_eq!(get_u32(f, "frame"), Some(0));
}

// ═══════════════════════════════════════════════════════════════════════
//  BGCOLOR
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bgcolor_key() {
    let r = parse("w=800&h=600&mode=pad&bgcolor=ff0000");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "canvas_color").as_deref(), Some("ff0000"));
}

// ═══════════════════════════════════════════════════════════════════════
//  RESAMPLING FILTERS
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn down_filter() {
    let r = parse("w=400&down.filter=lanczos");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "down_filter").as_deref(), Some("lanczos"));
}

#[test]
fn up_filter() {
    let r = parse("w=400&up.filter=mitchell");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "up_filter").as_deref(), Some("mitchell"));
}

// ═══════════════════════════════════════════════════════════════════════
//  SHARPENING
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn f_sharpen() {
    let r = parse("w=400&f.sharpen=15");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_f32(c, "unsharp_percent"), Some(15.0));
}

#[test]
fn sharpen_when() {
    let r = parse("w=400&sharpen_when=always");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "sharpen_when").as_deref(), Some("always"));
}

// ═══════════════════════════════════════════════════════════════════════
//  WHITESPACE TRIMMING
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn trim_threshold() {
    let r = parse("trim.threshold=80");
    let t = find_node(&r.instances, "zenpipe.crop_whitespace").expect("CropWhitespace node");
    assert_eq!(get_u32(t, "threshold"), Some(80));
}

#[test]
fn trim_percentpadding() {
    let r = parse("trim.percentpadding=0.5");
    let t = find_node(&r.instances, "zenpipe.crop_whitespace").expect("CropWhitespace node");
    let v = get_f32(t, "percent_padding").unwrap();
    assert!((v - 0.5).abs() < 0.01);
}

// ═══════════════════════════════════════════════════════════════════════
//  ROUND CORNERS
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn round_corners() {
    let r = parse("s.roundcorners=20");
    let rc = find_node(&r.instances, "zenpipe.round_corners").expect("RoundCorners node");
    assert_eq!(get_f32(rc, "radius"), Some(20.0));
}

// ═══════════════════════════════════════════════════════════════════════
//  COLOR SPACE
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn down_colorspace() {
    let r = parse("w=400&down.colorspace=srgb");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "scaling_colorspace").as_deref(), Some("srgb"));
}

// ═══════════════════════════════════════════════════════════════════════
//  FORMAT & QUALITY
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn format_key() {
    let r = parse("format=webp");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_str(q, "format").as_deref(), Some("webp"));
}

#[test]
fn format_thumbnail_alias() {
    let r = parse("thumbnail=png");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_str(q, "format").as_deref(), Some("png"));
}

#[test]
fn quality_profile() {
    let r = parse("qp=high");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_str(q, "profile").as_deref(), Some("high"));
}

#[test]
fn quality_legacy_fallback() {
    // Only QualityIntentNode should consume the bare `quality` key.
    let r = parse("quality=85");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_f32(q, "quality_fallback"), Some(85.0));
}

#[test]
fn lossless_key() {
    let r = parse("lossless=true");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    // lossless is a string field, not bool
    assert_eq!(get_str(q, "lossless").as_deref(), Some("true"));
}

#[test]
fn accept_webp() {
    let r = parse("accept.webp=true");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_bool(q, "allow_webp"), Some(true));
}

#[test]
fn accept_avif() {
    let r = parse("accept.avif=true");
    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent node");
    assert_eq!(get_bool(q, "allow_avif"), Some(true));
}

// ═══════════════════════════════════════════════════════════════════════
//  JPEG CODEC KEYS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "nodes-jpeg")]
#[test]
fn jpeg_quality() {
    let r = parse("jpeg.quality=85");
    let j = find_node(&r.instances, "zenjpeg.encode").expect("JPEG encode node");
    assert_eq!(get_f32(j, "quality"), Some(85.0));
}

#[cfg(feature = "nodes-jpeg")]
#[test]
fn jpeg_progressive() {
    let r = parse("jpeg.progressive=true");
    let j = find_node(&r.instances, "zenjpeg.encode").expect("JPEG encode node");
    let mode = get_str(j, "scan_mode");
    // progressive=true should map to scan_mode or similar
    assert!(
        mode.is_some() || get_bool(j, "progressive").is_some(),
        "jpeg.progressive should be consumed"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  PNG CODEC KEYS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "nodes-png")]
#[test]
fn png_quality() {
    let r = parse("png.quality=80");
    let p = find_node(&r.instances, "zenpng.encode").expect("PNG encode node");
    assert_eq!(get_u32(p, "png_quality"), Some(80));
}

#[cfg(feature = "nodes-png")]
#[test]
fn png_lossless() {
    let r = parse("png.lossless=true");
    let p = find_node(&r.instances, "zenpng.encode").expect("PNG encode node");
    assert_eq!(get_bool(p, "lossless"), Some(true));
}

#[cfg(feature = "nodes-png")]
#[test]
fn png_max_deflate() {
    // png.max_deflate is a boolean (not an effort level) — true enables maximum compression
    let r = parse("png.max_deflate=true");
    let p = find_node(&r.instances, "zenpng.encode").expect("PNG encode node");
    assert_eq!(get_bool(p, "max_deflate"), Some(true));
}

// ═══════════════════════════════════════════════════════════════════════
//  WEBP CODEC KEYS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "nodes-webp")]
#[test]
fn webp_quality() {
    let r = parse("webp.quality=75");
    let w = find_node(&r.instances, "zenwebp.encode_lossy").expect("WebP lossy encode node");
    assert_eq!(get_f32(w, "quality"), Some(75.0));
}

// ═══════════════════════════════════════════════════════════════════════
//  MATTE / BGCOLOR
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn matte_color() {
    let r = parse("w=800&matte=ffffff");
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "matte_color").as_deref(), Some("ffffff"));
}

// ═══════════════════════════════════════════════════════════════════════
//  NO UNCONSUMED KEY WARNINGS for known keys
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn common_query_no_warnings() {
    let r = parse("w=800&h=600&mode=crop&format=webp&qp=high");
    let unconsumed: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| matches!(w.kind, zennode::kv::KvWarningKind::UnrecognizedKey))
        .collect();
    assert!(
        unconsumed.is_empty(),
        "common keys should be fully consumed, but got warnings: {unconsumed:?}"
    );
}

#[test]
fn resize_with_filter_no_warnings() {
    let r = parse("w=400&down.filter=lanczos&f.sharpen=15");
    let unconsumed: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| matches!(w.kind, zennode::kv::KvWarningKind::UnrecognizedKey))
        .collect();
    assert!(
        unconsumed.is_empty(),
        "filter/sharpen keys should be consumed: {unconsumed:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  COMBINED QUERIES (real-world scenarios)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn typical_thumbnail_query() {
    let r = parse("w=200&h=200&mode=crop&format=webp&qp=good&accept.webp=true");
    assert!(!r.instances.is_empty(), "should produce nodes");
    assert!(find_node(&r.instances, "zenresize.constrain").is_some());
    assert!(find_node(&r.instances, "zencodecs.quality_intent").is_some());
}

#[test]
fn complex_query() {
    let r = parse(
        "w=1200&h=900&mode=pad&bgcolor=f0f0f0&down.filter=lanczos&f.sharpen=10&format=webp&qp=high&accept.webp=true&accept.avif=true",
    );
    let c = find_node(&r.instances, "zenresize.constrain").expect("Constrain");
    assert_eq!(get_u32(c, "w"), Some(1200));
    assert_eq!(get_str(c, "mode").as_deref(), Some("pad"));
    assert_eq!(get_str(c, "canvas_color").as_deref(), Some("f0f0f0"));
    assert_eq!(get_str(c, "down_filter").as_deref(), Some("lanczos"));
    assert_eq!(get_f32(c, "unsharp_percent"), Some(10.0));

    let q = find_node(&r.instances, "zencodecs.quality_intent").expect("QualityIntent");
    assert_eq!(get_str(q, "format").as_deref(), Some("webp"));
    assert_eq!(get_str(q, "profile").as_deref(), Some("high"));
    assert_eq!(get_bool(q, "allow_webp"), Some(true));
    assert_eq!(get_bool(q, "allow_avif"), Some(true));
}

// ═══════════════════════════════════════════════════════════════════════
//  CROP (RIAPI adapter)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn crop_default_units() {
    // crop=10,10,90,90 with default cropxunits=100, cropyunits=100
    let r = parse("crop=10,10,90,90");
    let c = find_node(&r.instances, "zenlayout.crop_percent").expect("CropPercent node");
    let x = get_f32(c, "x").unwrap();
    let y = get_f32(c, "y").unwrap();
    let w = get_f32(c, "w").unwrap();
    let h = get_f32(c, "h").unwrap();
    assert!((x - 0.1).abs() < 0.001, "x={x}, expected 0.1");
    assert!((y - 0.1).abs() < 0.001, "y={y}, expected 0.1");
    assert!((w - 0.8).abs() < 0.001, "w={w}, expected 0.8");
    assert!((h - 0.8).abs() < 0.001, "h={h}, expected 0.8");
}

#[test]
fn crop_c_shorthand() {
    // c=25,25,75,75 → auto units=100
    let r = parse("c=25,25,75,75");
    let c = find_node(&r.instances, "zenlayout.crop_percent").expect("CropPercent node");
    let x = get_f32(c, "x").unwrap();
    let y = get_f32(c, "y").unwrap();
    let w = get_f32(c, "w").unwrap();
    let h = get_f32(c, "h").unwrap();
    assert!((x - 0.25).abs() < 0.001, "x={x}, expected 0.25");
    assert!((y - 0.25).abs() < 0.001, "y={y}, expected 0.25");
    assert!((w - 0.5).abs() < 0.001, "w={w}, expected 0.5");
    assert!((h - 0.5).abs() < 0.001, "h={h}, expected 0.5");
}

#[test]
fn crop_custom_units() {
    // crop=0,0,200,200 with cropxunits=200, cropyunits=200 → full image
    let r = parse("crop=0,0,200,200&cropxunits=200&cropyunits=200");
    let c = find_node(&r.instances, "zenlayout.crop_percent").expect("CropPercent node");
    let x = get_f32(c, "x").unwrap();
    let y = get_f32(c, "y").unwrap();
    let w = get_f32(c, "w").unwrap();
    let h = get_f32(c, "h").unwrap();
    assert!((x - 0.0).abs() < 0.001, "x={x}, expected 0.0");
    assert!((y - 0.0).abs() < 0.001, "y={y}, expected 0.0");
    assert!((w - 1.0).abs() < 0.001, "w={w}, expected 1.0");
    assert!((h - 1.0).abs() < 0.001, "h={h}, expected 1.0");
}

// ═══════════════════════════════════════════════════════════════════════
//  GRAYSCALE & INVERT (RIAPI via #[kv])
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "nodes-filters")]
#[test]
fn s_grayscale() {
    let r = parse("s.grayscale=ntsc");
    // Should produce a zenfilters.grayscale node
    let g = find_node(&r.instances, "zenfilters.grayscale");
    assert!(g.is_some(), "s.grayscale should produce Grayscale node");
}

#[cfg(feature = "nodes-filters")]
#[test]
fn s_invert() {
    let r = parse("s.invert=true");
    let inv = find_node(&r.instances, "zenfilters.invert");
    assert!(inv.is_some(), "s.invert should produce Invert node");
}

// ═══════════════════════════════════════════════════════════════════════
//  C.GRAVITY (percentage focal point via expand_zen)
// ═══════════════════════════════════════════════════════════════════════

/// Helper: expand_zen with dummy source dimensions.
#[cfg(feature = "imageflow-compat")]
fn expand_zen(qs: &str) -> zenpipe::imageflow_compat::riapi::ExpandedRiapi {
    zenpipe::imageflow_compat::riapi::expand_zen(qs, 1000, 1000, None).unwrap()
}

#[cfg(feature = "imageflow-compat")]
fn find_expanded_node<'a>(
    nodes: &'a [Box<dyn NodeInstance>],
    schema_id: &str,
) -> Option<&'a dyn NodeInstance> {
    nodes
        .iter()
        .find(|n| n.schema().id == schema_id)
        .map(|n| n.as_ref())
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_sets_gravity_xy() {
    let r = expand_zen("w=800&h=600&mode=crop&c.gravity=30,70");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set");
    assert!((gx - 0.30).abs() < 0.001, "gravity_x={gx}, expected 0.30");
    assert!((gy - 0.70).abs() < 0.001, "gravity_y={gy}, expected 0.70");
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn anchor_still_works_via_expand_zen() {
    let r = expand_zen("w=800&h=600&mode=crop&anchor=topleft");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(get_str(c, "gravity").as_deref(), Some("topleft"));
    // gravity_x/gravity_y should NOT be set when only anchor is provided.
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "gravity_x should be None with anchor only"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_overrides_anchor() {
    let r = expand_zen("w=800&h=600&mode=crop&anchor=topleft&c.gravity=50,50");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set");
    // c.gravity=50,50 → center (0.5, 0.5) — should override anchor=topleft
    assert!((gx - 0.50).abs() < 0.001, "gravity_x={gx}, expected 0.50");
    assert!((gy - 0.50).abs() < 0.001, "gravity_y={gy}, expected 0.50");
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_clamped_to_0_100() {
    let r = expand_zen("w=400&h=400&mode=crop&c.gravity=-10,150");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set");
    assert!(
        (gx - 0.0).abs() < 0.001,
        "gravity_x={gx}, expected 0.0 (clamped)"
    );
    assert!(
        (gy - 1.0).abs() < 0.001,
        "gravity_y={gy}, expected 1.0 (clamped)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  C.GRAVITY EDGE CASES
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_single_value() {
    // c.gravity=30 (only one value, no comma) — should return None, no panic
    let r = expand_zen("w=400&h=400&mode=crop&c.gravity=30");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "single-value c.gravity should be ignored"
    );
    assert_eq!(
        get_f32(c, "gravity_y"),
        None,
        "single-value c.gravity should be ignored"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_non_numeric() {
    // c.gravity=abc (non-numeric) — should return None, no panic
    let r = expand_zen("w=400&h=400&mode=crop&c.gravity=abc");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "non-numeric c.gravity should be ignored"
    );
    assert_eq!(
        get_f32(c, "gravity_y"),
        None,
        "non-numeric c.gravity should be ignored"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_empty_value() {
    // c.gravity= (empty value) — should return None, no panic
    let r = expand_zen("w=400&h=400&mode=crop&c.gravity=");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "empty c.gravity should be ignored"
    );
    assert_eq!(
        get_f32(c, "gravity_y"),
        None,
        "empty c.gravity should be ignored"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_too_many_values() {
    // c.gravity=30,70,50 (three values) — should return None, no panic
    let r = expand_zen("w=400&h=400&mode=crop&c.gravity=30,70,50");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "three-value c.gravity should be ignored"
    );
    assert_eq!(
        get_f32(c, "gravity_y"),
        None,
        "three-value c.gravity should be ignored"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_absent() {
    // No c.gravity key at all — should return None
    let r = expand_zen("w=400&h=400&mode=crop");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_f32(c, "gravity_x"),
        None,
        "absent c.gravity should leave gravity_x None"
    );
    assert_eq!(
        get_f32(c, "gravity_y"),
        None,
        "absent c.gravity should leave gravity_y None"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_after_bare_key() {
    // c.gravity after a bare key (no =) — should still be found
    let r = expand_zen("w=400&h=400&mode=crop&barekey&c.gravity=30,70");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set despite preceding bare key");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set despite preceding bare key");
    assert!((gx - 0.30).abs() < 0.001, "gravity_x={gx}, expected 0.30");
    assert!((gy - 0.70).abs() < 0.001, "gravity_y={gy}, expected 0.70");
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_gravity_no_unconsumed_warning() {
    let r = expand_zen("w=800&h=600&mode=crop&c.gravity=30,70");
    // The c.gravity key should NOT produce an "unrecognized key" warning.
    let gravity_warnings: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| w.contains("c.gravity"))
        .collect();
    assert!(
        gravity_warnings.is_empty(),
        "c.gravity should not produce warnings, got: {gravity_warnings:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  C.FOCUS (smart crop: rects, points, keywords)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_rect_injects_analyze_before_constrain() {
    let r = expand_zen("c.focus=20,30,80,90&w=800&h=600&mode=crop");
    // Should have SmartCropAnalyze BEFORE Constrain.
    let analyze_idx = r
        .nodes
        .iter()
        .position(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    let constrain_idx = r
        .nodes
        .iter()
        .position(|n| n.schema().id == "zenresize.constrain");
    assert!(
        analyze_idx.is_some(),
        "SmartCropAnalyze node should be present"
    );
    assert!(constrain_idx.is_some(), "Constrain node should be present");
    assert!(
        analyze_idx.unwrap() < constrain_idx.unwrap(),
        "SmartCropAnalyze should come before Constrain"
    );

    // Verify the Analyze node has the correct parameters.
    let a = find_expanded_node(&r.nodes, "zenpipe.smart_crop_analyze").unwrap();
    assert_eq!(get_str(a, "rects_csv").as_deref(), Some("20,30,80,90"));
    assert_eq!(get_u32(a, "target_w"), Some(800));
    assert_eq!(get_u32(a, "target_h"), Some(600));
    assert_eq!(get_bool(a, "zoom"), Some(false));
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_point_sets_gravity_on_constrain() {
    let r = expand_zen("c.focus=50,30&w=400&h=400&mode=crop");
    // Point should set gravity, NOT inject Analyze.
    let analyze = r
        .nodes
        .iter()
        .find(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_none(),
        "Point c.focus should NOT inject Analyze node"
    );

    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set");
    assert!((gx - 0.50).abs() < 0.001, "gravity_x={gx}, expected 0.50");
    assert!((gy - 0.30).abs() < 0.001, "gravity_y={gy}, expected 0.30");
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_faces_without_feature_is_noop() {
    let r = expand_zen("c.focus=faces&w=800&h=600&mode=crop");
    // Without nodes-faces feature, faces keyword should be silently ignored.
    let analyze = r
        .nodes
        .iter()
        .find(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_none(),
        "c.focus=faces should NOT inject Analyze node without ML feature"
    );
    // Should still have the Constrain node.
    assert!(
        find_expanded_node(&r.nodes, "zenresize.constrain").is_some(),
        "Constrain node should still be present"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_without_target_dims_is_noop() {
    // No w/h means no aspect ratio, so c.focus rects should be ignored.
    let r = expand_zen("c.focus=20,30,80,90");
    let analyze = r
        .nodes
        .iter()
        .find(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_none(),
        "c.focus rects without target dims should be ignored"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_multiple_rects_all_converted() {
    let r = expand_zen("c.focus=20,30,80,90,10,10,40,40&w=800&h=600&mode=crop");
    let a = find_expanded_node(&r.nodes, "zenpipe.smart_crop_analyze")
        .expect("SmartCropAnalyze node should be present");
    assert_eq!(
        get_str(a, "rects_csv").as_deref(),
        Some("20,30,80,90,10,10,40,40"),
        "All rects should be preserved in CSV"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_zoom_true_propagates() {
    let r = expand_zen("c.focus=20,30,80,90&c.zoom=true&w=800&h=600&mode=crop");
    let a = find_expanded_node(&r.nodes, "zenpipe.smart_crop_analyze")
        .expect("SmartCropAnalyze node should be present");
    assert_eq!(
        get_bool(a, "zoom"),
        Some(true),
        "c.zoom=true should set zoom=true"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_zoom_false_propagates() {
    let r = expand_zen("c.focus=20,30,80,90&c.zoom=false&w=800&h=600&mode=crop");
    let a = find_expanded_node(&r.nodes, "zenpipe.smart_crop_analyze")
        .expect("SmartCropAnalyze node should be present");
    assert_eq!(
        get_bool(a, "zoom"),
        Some(false),
        "c.zoom=false should set zoom=false"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_finalmode_propagates() {
    let r = expand_zen("c.focus=50,30&c.finalmode=pad&w=400&h=400&mode=crop");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    assert_eq!(
        get_str(c, "mode").as_deref(),
        Some("pad"),
        "c.finalmode=pad should override mode on Constrain"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_with_c_gravity_both_apply() {
    // c.focus point takes precedence over c.gravity for gravity values.
    let r = expand_zen("c.focus=60,40&c.gravity=30,70&w=400&h=400&mode=crop");
    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set");
    // c.focus=60,40 should override c.gravity=30,70
    assert!(
        (gx - 0.60).abs() < 0.001,
        "gravity_x={gx}, expected 0.60 (c.focus overrides c.gravity)"
    );
    assert!(
        (gy - 0.40).abs() < 0.001,
        "gravity_y={gy}, expected 0.40 (c.focus overrides c.gravity)"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_rects_with_c_gravity_both_apply() {
    // c.focus rects inject Analyze, c.gravity sets gravity on Constrain.
    let r = expand_zen("c.focus=20,30,80,90&c.gravity=30,70&w=800&h=600&mode=crop");
    // Should have both Analyze and Constrain with gravity.
    let analyze = find_expanded_node(&r.nodes, "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_some(),
        "SmartCropAnalyze should be present for rects"
    );

    let c = find_expanded_node(&r.nodes, "zenresize.constrain").expect("Constrain node");
    let gx = get_f32(c, "gravity_x").expect("gravity_x should be set from c.gravity");
    let gy = get_f32(c, "gravity_y").expect("gravity_y should be set from c.gravity");
    assert!((gx - 0.30).abs() < 0.001, "gravity_x={gx}, expected 0.30");
    assert!((gy - 0.70).abs() < 0.001, "gravity_y={gy}, expected 0.70");
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_auto_without_feature_is_noop() {
    let r = expand_zen("c.focus=auto&w=800&h=600&mode=crop");
    let analyze = r
        .nodes
        .iter()
        .find(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_none(),
        "c.focus=auto should NOT inject Analyze node without ML feature"
    );
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_invalid_value_no_crash() {
    // 3 values (not 2 or multiple of 4) — should be silently ignored.
    let r = expand_zen("c.focus=20,30,80&w=800&h=600&mode=crop");
    let analyze = r
        .nodes
        .iter()
        .find(|n| n.schema().id == "zenpipe.smart_crop_analyze");
    assert!(
        analyze.is_none(),
        "Invalid c.focus should not produce Analyze node"
    );
    // Constrain should still be present.
    assert!(find_expanded_node(&r.nodes, "zenresize.constrain").is_some());
}

#[cfg(feature = "imageflow-compat")]
#[test]
fn c_focus_no_unconsumed_warnings() {
    let r = expand_zen("w=800&h=600&mode=crop&c.focus=20,30,80,90&c.zoom=true&c.finalmode=pad");
    let focus_warnings: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| w.contains("c.focus") || w.contains("c.zoom") || w.contains("c.finalmode"))
        .collect();
    assert!(
        focus_warnings.is_empty(),
        "c.focus/c.zoom/c.finalmode should not produce warnings, got: {focus_warnings:?}"
    );
}
