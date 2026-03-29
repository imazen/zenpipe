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
    instances.iter().find(|n| n.schema().id == schema_id).map(|n| n.as_ref())
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
    assert!(constrain_has || qi_has, "dpr should be consumed by Constrain or QualityIntent");
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

// NOTE: `autorotate=true` (boolean) doesn't work with Orient's i32 field.
// This is a known gap — autorotate needs a separate bool field or adapter.
// imageflow_riapi handles autorotate as a flag that reads EXIF at decode time.

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
    assert!(mode.is_some() || get_bool(j, "progressive").is_some(),
        "jpeg.progressive should be consumed");
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
    let unconsumed: Vec<_> = r.warnings.iter()
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
    let unconsumed: Vec<_> = r.warnings.iter()
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
    let r = parse("w=1200&h=900&mode=pad&bgcolor=f0f0f0&down.filter=lanczos&f.sharpen=10&format=webp&qp=high&accept.webp=true&accept.avif=true");
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
