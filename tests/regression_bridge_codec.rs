//! Regression tests for RIAPI bridge, codec capabilities, and registry export.
//!
//! Covers:
//! - RIAPI `qp`, `qp.dpr`, `format=auto` wiring through zen-native bridge
//! - Focal point / percentage gravity from RIAPI
//! - Runtime codec capability queries (AllowedFormats)
//! - Registry key export for v2 and zen backends

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

fn get_bool(node: &dyn NodeInstance, param: &str) -> Option<bool> {
    node.get_param(param)?.as_bool()
}

// ═══════════════════════════════════════════════════════════════════════
//  Issue 5: qp, qp.dpr, format=auto through RIAPI bridge
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn riapi_qp_medium_produces_quality_intent() {
    let r = parse("qp=medium");
    let q = find_node(&r.instances, "zencodecs.quality_intent")
        .expect("qp=medium should produce QualityIntent node");
    assert_eq!(
        get_str(q, "profile").as_deref(),
        Some("medium"),
        "profile should be 'medium'"
    );
}

#[test]
fn riapi_qp_high_produces_quality_intent() {
    let r = parse("qp=high");
    let q = find_node(&r.instances, "zencodecs.quality_intent")
        .expect("qp=high should produce QualityIntent node");
    assert_eq!(get_str(q, "profile").as_deref(), Some("high"));
}

#[test]
fn riapi_qp_lowest_through_highest() {
    for profile in &[
        "lowest",
        "low",
        "medium_low",
        "medium",
        "good",
        "high",
        "highest",
        "lossless",
    ] {
        let r = parse(&format!("qp={profile}"));
        let q = find_node(&r.instances, "zencodecs.quality_intent")
            .unwrap_or_else(|| panic!("qp={profile} should produce QualityIntent node"));
        assert_eq!(
            get_str(q, "profile").as_deref(),
            Some(*profile),
            "profile mismatch for qp={profile}"
        );
    }
}

#[test]
fn riapi_qp_dpr_produces_dpr_param() {
    let r = parse("qp=good&qp.dpr=2");
    let q = find_node(&r.instances, "zencodecs.quality_intent")
        .expect("qp.dpr should produce QualityIntent node");
    assert_eq!(
        get_f32(q, "dpr"),
        Some(2.0),
        "dpr should be 2.0 from qp.dpr=2"
    );
}

#[test]
fn riapi_dpr_alias_produces_dpr_param() {
    // "dpr" is an alias for "qp.dpr"
    let r = parse("dpr=1.5");
    let q = find_node(&r.instances, "zencodecs.quality_intent");
    let c = find_node(&r.instances, "zenresize.constrain");
    // dpr is consumed by QualityIntent as dpr and/or by Constrain as zoom
    let qi_has = q.and_then(|q| get_f32(q, "dpr")).is_some();
    let constrain_has = c.and_then(|c| get_f32(c, "zoom")).is_some();
    assert!(
        qi_has || constrain_has,
        "dpr=1.5 should be consumed by QualityIntent (dpr) or Constrain (zoom)"
    );
}

#[test]
fn riapi_format_auto_empty_string() {
    // format= (empty) is a recognized key — the registry will consume it and
    // create a QualityIntentNode with format="" (auto-select).
    let r = parse("format=");
    let q = find_node(&r.instances, "zencodecs.quality_intent");
    assert!(
        q.is_some(),
        "format= (empty) should produce a quality_intent node (format is a recognized key)"
    );
    let q = q.unwrap();
    let fmt = get_str(q, "format").unwrap_or_default();
    assert!(
        fmt.is_empty(),
        "format= (empty) should produce empty format field, got '{fmt}'"
    );
}

#[test]
fn riapi_format_webp_produces_quality_intent() {
    let r = parse("format=webp");
    let q = find_node(&r.instances, "zencodecs.quality_intent")
        .expect("format=webp should produce QualityIntent node");
    assert_eq!(
        get_str(q, "format").as_deref(),
        Some("webp"),
        "format should be 'webp'"
    );
}

#[test]
fn riapi_combined_qp_dpr_format() {
    let r = parse("qp=medium&qp.dpr=2&format=webp&accept.webp=true");
    let unconsumed: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| matches!(w.kind, zennode::kv::KvWarningKind::UnrecognizedKey))
        .collect();
    assert!(
        unconsumed.is_empty(),
        "qp + qp.dpr + format + accept.webp should all be consumed: {unconsumed:?}"
    );

    let q = find_node(&r.instances, "zencodecs.quality_intent")
        .expect("combined query should produce QualityIntent");
    assert_eq!(get_str(q, "profile").as_deref(), Some("medium"));
    assert_eq!(get_f32(q, "dpr"), Some(2.0));
    assert_eq!(get_str(q, "format").as_deref(), Some("webp"));
    assert_eq!(get_bool(q, "allow_webp"), Some(true));
}

// ═══════════════════════════════════════════════════════════════════════
//  Issue 6: focal point / percentage gravity from RIAPI
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn riapi_anchor_topleft_sets_gravity() {
    let r = parse("w=800&h=600&mode=crop&anchor=topleft");
    let c = find_node(&r.instances, "zenresize.constrain")
        .expect("Constrain node should exist");
    assert_eq!(
        get_str(c, "gravity").as_deref(),
        Some("topleft"),
        "anchor=topleft should map to gravity=topleft"
    );
}

#[test]
fn riapi_anchor_bottomright_sets_gravity() {
    let r = parse("w=800&h=600&mode=crop&anchor=bottomright");
    let c = find_node(&r.instances, "zenresize.constrain")
        .expect("Constrain node should exist");
    assert_eq!(
        get_str(c, "gravity").as_deref(),
        Some("bottomright"),
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  Issue 7: runtime codec capabilities
//  (codec_info module requires the json-schema feature)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "json-schema")]
#[test]
fn list_codecs_returns_known_formats() {
    let registry = zencodecs::AllowedFormats::all();
    let codecs = zenpipe::codec_info::list_codecs(&registry);
    assert!(
        codecs.len() >= 5,
        "should have at least 5 known formats, got {}",
        codecs.len()
    );

    // Verify at least JPEG and PNG are present.
    let names: Vec<&str> = codecs.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"jpeg"), "should include JPEG: {names:?}");
    assert!(names.contains(&"png"), "should include PNG: {names:?}");
}

#[cfg(feature = "json-schema")]
#[test]
fn list_codecs_reports_capability_flags() {
    let registry = zencodecs::AllowedFormats::all();
    let codecs = zenpipe::codec_info::list_codecs(&registry);

    // can_decode and can_encode depend on compiled-in codec features.
    // With default features, no codecs may be compiled in. But the list
    // should still report capability flags (false in that case).
    for codec in &codecs {
        // Every codec should have a non-empty MIME type and extension.
        assert!(!codec.mime_type.is_empty(), "{} missing mime_type", codec.name);
        assert!(!codec.extension.is_empty(), "{} missing extension", codec.name);
    }

    // The decodable/encodable lists should be consistent: if a format is
    // decodable via AllowedFormats, it should show can_decode=true.
    // This is a structural check, not a feature-dependent check.
    let gif = codecs.iter().find(|c| c.name == "gif");
    if let Some(gif) = gif {
        // GIF is in compiled_both(), so decode and encode are always
        // enabled together. They may both be false if the gif feature
        // is not compiled in, but they must never disagree.
        assert_eq!(
            gif.can_decode, gif.can_encode,
            "GIF decode/encode capability must be consistent (both true or both false)"
        );
    }
}

#[cfg(feature = "json-schema")]
#[test]
fn list_codecs_gif_supports_animation() {
    let registry = zencodecs::AllowedFormats::all();
    let codecs = zenpipe::codec_info::list_codecs(&registry);
    let gif = codecs
        .iter()
        .find(|c| c.name == "gif")
        .expect("GIF should be in codec list");
    assert!(gif.supports_animation, "GIF should report animation support");
    assert!(gif.supports_alpha, "GIF should report alpha support");
}

#[cfg(feature = "json-schema")]
#[test]
fn list_codecs_jpeg_no_alpha_no_animation() {
    let registry = zencodecs::AllowedFormats::all();
    let codecs = zenpipe::codec_info::list_codecs(&registry);
    let jpeg = codecs
        .iter()
        .find(|c| c.name == "jpeg")
        .expect("JPEG should be in codec list");
    assert!(!jpeg.supports_alpha, "JPEG should not report alpha support");
    assert!(
        !jpeg.supports_animation,
        "JPEG should not report animation support"
    );
}

#[cfg(feature = "json-schema")]
#[test]
fn list_codecs_json_is_valid() {
    let registry = zencodecs::AllowedFormats::all();
    let json = zenpipe::codec_info::list_codecs_json(&registry);
    let codecs = json["codecs"]
        .as_array()
        .expect("JSON should have 'codecs' array");
    assert!(
        codecs.len() >= 5,
        "JSON codecs array should have >= 5 entries"
    );

    // Each entry should have required fields.
    for entry in codecs {
        assert!(entry["name"].is_string(), "entry missing 'name'");
        assert!(entry["mime_type"].is_string(), "entry missing 'mime_type'");
        assert!(
            entry["can_decode"].is_boolean(),
            "entry missing 'can_decode'"
        );
        assert!(
            entry["can_encode"].is_boolean(),
            "entry missing 'can_encode'"
        );
    }
}

#[cfg(feature = "json-schema")]
#[test]
fn detect_format_gif_header() {
    // GIF89a magic bytes.
    let gif_header = b"GIF89a\x01\x00\x01\x00\x00\xff\x00";
    let result = zenpipe::codec_info::detect_format(gif_header);
    assert!(result.is_some(), "should detect GIF from magic bytes");
    assert_eq!(result.unwrap().name, "gif");
}

// ═══════════════════════════════════════════════════════════════════════
//  Issue 8: registry key export for v2 and zen backends
// ═══════════════════════════════════════════════════════════════════════

#[cfg(feature = "json-schema")]
#[test]
fn export_querystring_keys_includes_zen_nodes() {
    let keys = zenpipe::schema_export::export_querystring_keys();
    let nodes = keys
        .get("nodes")
        .expect("should have 'nodes' key")
        .as_object()
        .expect("'nodes' should be an object");

    // Zen-native nodes should be present.
    assert!(
        nodes.contains_key("zenresize.constrain"),
        "should include zenresize.constrain"
    );
    assert!(
        nodes.contains_key("zencodecs.quality_intent"),
        "should include zencodecs.quality_intent"
    );
}

#[cfg(feature = "json-schema")]
#[test]
fn export_querystring_keys_has_qp_key() {
    let keys = zenpipe::schema_export::export_querystring_keys();
    let nodes = keys.get("nodes").unwrap();
    let qi = nodes
        .get("zencodecs.quality_intent")
        .expect("quality_intent should exist");
    let ks = qi
        .get("keys")
        .expect("should have 'keys'")
        .as_array()
        .expect("'keys' should be array");

    let has_qp = ks.iter().any(|k| {
        k.get("key")
            .and_then(|v| v.as_str())
            .map(|s| s == "qp")
            .unwrap_or(false)
    });
    assert!(has_qp, "quality_intent should export the 'qp' key");
}

#[cfg(feature = "json-schema")]
#[test]
fn export_querystring_schema_has_qp_and_format_keys() {
    let qs = zenpipe::schema_export::export_querystring_schema();
    let props = qs
        .get("properties")
        .expect("should have 'properties'")
        .as_object()
        .expect("'properties' should be object");

    assert!(props.contains_key("qp"), "should have 'qp' key");
    assert!(props.contains_key("format"), "should have 'format' key");
    assert!(
        props.contains_key("qp.dpr") || props.contains_key("dpr"),
        "should have 'qp.dpr' or 'dpr' key"
    );
}

#[cfg(feature = "json-schema")]
#[test]
fn export_querystring_keys_includes_kv_annotated_nodes() {
    let keys = zenpipe::schema_export::export_querystring_keys();
    let nodes = keys
        .get("nodes")
        .unwrap()
        .as_object()
        .unwrap();

    // Only nodes with #[kv(...)] annotations appear in the querystring key
    // registry. zenlayout.crop has no kv keys (it's JSON-only), but
    // zenlayout.orient has #[kv("srotate")] and zenresize.constrain has many.
    let has_orient = nodes.contains_key("zenlayout.orient");
    let has_constrain = nodes.contains_key("zenresize.constrain");

    assert!(has_orient, "should include zenlayout.orient (has srotate kv key)");
    assert!(has_constrain, "should include zenresize.constrain (has w/h/mode kv keys)");

    // Verify the keys registry covers both zenpipe-owned and zencodecs-owned nodes.
    let node_ids: Vec<&str> = nodes.keys().map(|k| k.as_str()).collect();
    let has_zenpipe_node = node_ids.iter().any(|id| id.starts_with("zenresize.") || id.starts_with("zenlayout."));
    let has_zencodecs_node = node_ids.iter().any(|id| id.starts_with("zencodecs."));
    assert!(has_zenpipe_node, "should include at least one zenpipe-owned node");
    assert!(has_zencodecs_node, "should include at least one zencodecs-owned node");
}

#[cfg(feature = "json-schema")]
#[test]
fn export_all_schemas_non_empty() {
    let schemas = zenpipe::schema_export::export_all();
    assert!(
        schemas.node_schemas.get("$defs").is_some(),
        "node_schemas should have $defs"
    );
    assert!(
        schemas.querystring_keys.get("nodes").is_some(),
        "querystring_keys should have nodes"
    );
    assert!(
        schemas.querystring_schema.get("properties").is_some(),
        "querystring_schema should have properties"
    );
}

// ═══════════════════════════════════════════════════════════════════════
//  Registry integration: full_registry covers all node sources
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn full_registry_has_nodes() {
    let registry = registry();
    let count = registry.all().len();
    assert!(
        count >= 10,
        "full_registry should have at least 10 node definitions, got {count}"
    );
}

#[test]
fn full_registry_includes_quality_intent() {
    let registry = registry();
    let has_qi = registry
        .all()
        .iter()
        .any(|def| def.schema().id == "zencodecs.quality_intent");
    assert!(has_qi, "full_registry should include zencodecs.quality_intent");
}

#[test]
fn full_registry_includes_constrain() {
    let registry = registry();
    let has_constrain = registry
        .all()
        .iter()
        .any(|def| def.schema().id == "zenresize.constrain");
    assert!(has_constrain, "full_registry should include zenresize.constrain");
}
