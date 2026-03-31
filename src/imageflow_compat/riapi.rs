//! RIAPI querystring expansion — dual implementation.
//!
//! Two paths for expanding RIAPI querystrings into zennode instances:
//!
//! - **Legacy path** (`expand_legacy`): Uses `imageflow_riapi::ir4::Ir4Expand` to parse
//!   the full IR4 querystring and produce v2 `Node` steps, then translates via `translate.rs`.
//!   Battle-tested v2-compatible path with full 68-key coverage.
//!
//! - **Zen-native path** (`expand_zen`): Uses `zenlayout::riapi::parse()` for geometry,
//!   then feeds non-geometry keys through `zennode::NodeRegistry::from_querystring()`
//!   so each codec/filter crate handles its own keys. Modular and extensible.
//!
//! Both produce `Vec<Box<dyn NodeInstance>>` that zenpipe can execute.

use zennode::{NodeDef, NodeInstance, NodeRegistry};

use super::preset_map::PresetMapping;
use super::translate::{self, TranslateError};

/// Which RIAPI parser to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RiapiEngine {
    /// Use imageflow_riapi (legacy v2 parser, full coverage).
    #[default]
    Legacy,
    /// Use zenlayout::riapi + zennode registry (modular zen-native parser).
    ZenNative,
}

/// Result of expanding a RIAPI querystring.
pub struct ExpandedRiapi {
    /// Zenode instances for pixel-processing operations.
    pub nodes: Vec<Box<dyn NodeInstance>>,
    /// Encoder configuration derived from format/quality keys.
    pub preset: Option<PresetMapping>,
    /// Warnings from parsing (unknown keys, deprecated keys, etc.).
    pub warnings: Vec<String>,
}

/// Expand a RIAPI querystring into zennode instances using the legacy parser.
///
/// Uses `imageflow_riapi::ir4::Ir4Expand` to parse the full IR4 querystring,
/// produces v2 `Node` steps (with source dimensions for layout), then translates
/// to zennode instances.
///
/// # Arguments
/// * `querystring` — the raw querystring (without leading `?`)
/// * `source_w` — source image width (needed for layout computation)
/// * `source_h` — source image height
/// * `source_mime` — source MIME type (for format auto-detection)
/// * `source_lossless` — whether source is lossless
/// * `exif_flag` — EXIF orientation flag (1-8)
/// * `encode_io_id` — io_id for the encode output (None if not encoding)
pub fn expand_legacy(
    querystring: &str,
    source_w: i32,
    source_h: i32,
    source_mime: Option<&str>,
    source_lossless: bool,
    exif_flag: u8,
    encode_io_id: Option<i32>,
) -> Result<ExpandedRiapi, TranslateError> {
    use imageflow_riapi::ir4::*;
    use imageflow_types as s;

    let command = Ir4Command::QueryString(querystring.to_string());

    let expand = Ir4Expand {
        i: command,
        source: Ir4SourceFrameInfo {
            w: source_w,
            h: source_h,
            fmt: s::PixelFormat::Bgra32,
            original_mime: source_mime.map(|s| s.to_string()),
            lossless: source_lossless,
        },
        reference_width: source_w,
        reference_height: source_h,
        encode_id: encode_io_id,
        watermarks: None,
    };

    let result = expand
        .expand_steps()
        .map_err(|e| TranslateError::InvalidParam(format!("RIAPI expansion error: {e:?}")))?;

    let steps = result.steps.unwrap_or_default();
    let mut warnings: Vec<String> = result
        .parse_warnings
        .iter()
        .map(|w| format!("{w:?}"))
        .collect();

    // Translate v2 Node steps → zennode instances.
    // RIAPI path never produces Watermark nodes, so pass empty io_buffers.
    let pipeline = translate::translate_nodes(&steps, &std::collections::HashMap::new())?;

    let mut nodes = pipeline.nodes;

    // Apply c.focus / c.zoom / c.finalmode post-processing (shared with execute.rs).
    apply_c_focus_postprocessing(&mut nodes, querystring);

    Ok(ExpandedRiapi {
        nodes,
        preset: pipeline.preset,
        warnings,
    })
}

/// Expand a RIAPI querystring using the zen-native parser.
///
/// Uses `zenlayout::riapi::parse()` for geometry, then feeds remaining
/// keys through zennode registry for filter/codec nodes.
///
/// This is the modular path — each crate only handles its own keys.
/// Currently handles fewer keys than `expand_legacy` but is more extensible.
pub fn expand_zen(
    querystring: &str,
    source_w: u32,
    source_h: u32,
    exif_flag: Option<u8>,
) -> Result<ExpandedRiapi, TranslateError> {
    // 1. Build a registry with all zen-native nodes.
    let mut registry = NodeRegistry::new();
    crate::zennode_defs::register(&mut registry);
    zenfilters::zennode_defs::register_all(&mut registry);
    // zencodecs quality intent node — auto-generated name from derive.
    registry.register(&zencodecs::zennode_defs::QUALITY_INTENT_NODE_NODE);

    // 2. Parse via zennode's unified querystring dispatch.
    // Each registered node claims its own keys via #[kv(...)] annotations.
    let kv_result = registry.from_querystring(querystring);

    // 2b. Parse compound keys from the raw querystring — these map to
    // multiple fields or require special handling beyond #[kv].
    let c_gravity = parse_c_gravity(querystring);
    let c_focus = parse_c_focus(querystring);
    let c_zoom = parse_c_zoom(querystring);
    let c_finalmode = parse_c_finalmode(querystring);

    let mut nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
    let mut preset = None;
    let mut warnings: Vec<String> = Vec::new();

    // Keys we handle manually — suppress "unrecognized key" warnings for these.
    const MANUAL_KEYS: &[&str] = &["c.gravity", "c.focus", "c.zoom", "c.finalmode"];

    for w in &kv_result.warnings {
        if MANUAL_KEYS.iter().any(|k| w.key.eq_ignore_ascii_case(k)) {
            continue;
        }
        warnings.push(format!("{}: {}", w.key, w.message));
    }

    // 3. Separate pixel-processing nodes from codec intent nodes.
    for mut inst in kv_result.instances {
        let schema_id = inst.schema().id;
        if schema_id == "zencodecs.quality_intent" {
            // Extract codec intent from QualityIntentNode.
            if let Some(qin) = inst
                .as_any()
                .downcast_ref::<zencodecs::zennode_defs::QualityIntentNode>()
            {
                let intent = qin.to_codec_intent();
                preset = Some(PresetMapping {
                    intent: intent.clone(),
                    explicit_format: match &intent.format {
                        Some(zencodecs::FormatChoice::Specific(f)) => Some(*f),
                        _ => None,
                    },
                });
            }
        } else {
            if schema_id == "zenresize.constrain" {
                // Apply c.gravity to the Constrain node.
                if let Some((gx, gy)) = c_gravity {
                    inst.set_param("gravity_x", zennode::ParamValue::F32(gx));
                    inst.set_param("gravity_y", zennode::ParamValue::F32(gy));
                }

                // Apply c.focus point — overrides c.gravity if both present.
                if let Some(CFocusParsed::Point(fx, fy)) = &c_focus {
                    let gx = (fx / 100.0).clamp(0.0, 1.0);
                    let gy = (fy / 100.0).clamp(0.0, 1.0);
                    inst.set_param("gravity_x", zennode::ParamValue::F32(gx));
                    inst.set_param("gravity_y", zennode::ParamValue::F32(gy));
                }

                // Apply c.finalmode — override the Constrain mode.
                if let Some(ref fm) = c_finalmode {
                    inst.set_param("mode", zennode::ParamValue::Str(fm.clone()));
                }
            }
            nodes.push(inst);
        }
    }

    // 4. Inject SmartCropAnalyze before Constrain for c.focus rects.
    if let Some(CFocusParsed::Rects(ref rects)) = c_focus {
        // Find the Constrain node and extract target dimensions.
        let constrain_idx = nodes
            .iter()
            .position(|n| n.schema().id == "zenresize.constrain");

        if let Some(idx) = constrain_idx {
            let constrain = &nodes[idx];
            let target_w = constrain
                .get_param("w")
                .and_then(|v| v.as_u32())
                .unwrap_or(0);
            let target_h = constrain
                .get_param("h")
                .and_then(|v| v.as_u32())
                .unwrap_or(0);

            // Only inject if we have target dimensions (need aspect ratio).
            if target_w > 0 && target_h > 0 {
                // Build CSV from rects.
                let csv: String = rects
                    .iter()
                    .flat_map(|r| r.iter().copied())
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(",");

                let mut analyze = crate::zennode_defs::SmartCropAnalyze::default();
                analyze.rects_csv = csv;
                analyze.target_w = target_w;
                analyze.target_h = target_h;
                analyze.zoom = c_zoom.unwrap_or(false);

                // Insert BEFORE the Constrain node.
                nodes.insert(idx, Box::new(analyze));
            }
        }
    }

    // Faces / Auto: silently ignored without nodes-faces feature.
    // No error, no Analyze node — normal crop proceeds.
    #[cfg(feature = "nodes-faces")]
    if matches!(c_focus, Some(CFocusParsed::Faces | CFocusParsed::Auto)) {
        // TODO: Build ML Analyze closure when nodes-faces is available.
        // For now, silently ignored even with the feature.
    }

    Ok(ExpandedRiapi {
        nodes,
        preset,
        warnings,
    })
}

/// Parse `c.gravity=x,y` from a raw querystring.
///
/// Returns `Some((gx, gy))` in 0.0–1.0 range (percentage / 100, clamped).
/// Matches zenlayout's parsing: `c.gravity=30,70` → `(0.30, 0.70)`.
fn parse_c_gravity(querystring: &str) -> Option<(f32, f32)> {
    for part in querystring.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("c.gravity") {
            let (x_str, y_str) = value.split_once(',')?;
            let x: f32 = x_str.trim().parse().ok()?;
            let y: f32 = y_str.trim().parse().ok()?;
            return Some(((x / 100.0).clamp(0.0, 1.0), (y / 100.0).clamp(0.0, 1.0)));
        }
    }
    None
}

// ─── c.focus / c.zoom / c.finalmode parsing ───

/// Parsed `c.focus` value.
#[derive(Debug, Clone, PartialEq)]
enum CFocusParsed {
    /// Focal point (x, y) in percentage coords (0-100). 2-value form.
    Point(f32, f32),
    /// Focus rectangles [x1, y1, x2, y2] in percentage coords (0-100).
    Rects(Vec<[f32; 4]>),
    /// Keyword: trigger face detection (requires ML backend).
    Faces,
    /// Keyword: trigger faces + saliency (requires ML backend).
    Auto,
}

/// Apply `c.focus` / `c.zoom` / `c.finalmode` post-processing to a list of zennode instances.
///
/// This is called after RIAPI expansion (both legacy and zen-native paths) and also
/// by `execute.rs` after `translate_nodes()`. Shared logic to avoid duplication.
///
/// - `Point(x,y)` → sets gravity on Constrain node
/// - `Rects(...)` → injects SmartCropAnalyze before Constrain
/// - `Faces`/`Auto` → silently ignored without `nodes-faces` feature
/// - `c.finalmode` → overrides Constrain mode
pub(crate) fn apply_c_focus_postprocessing(
    nodes: &mut Vec<Box<dyn NodeInstance>>,
    querystring: &str,
) {
    let c_focus = parse_c_focus(querystring);
    let c_zoom = parse_c_zoom(querystring);
    let c_finalmode = parse_c_finalmode(querystring);

    // Apply c.focus point and c.finalmode to the Constrain node.
    for inst in nodes.iter_mut() {
        let schema_id = inst.schema().id;
        if schema_id == "zenresize.constrain" || schema_id == "zenlayout.constrain" {
            if let Some(CFocusParsed::Point(fx, fy)) = &c_focus {
                let gx = (fx / 100.0).clamp(0.0, 1.0);
                let gy = (fy / 100.0).clamp(0.0, 1.0);
                inst.set_param("gravity_x", zennode::ParamValue::F32(gx));
                inst.set_param("gravity_y", zennode::ParamValue::F32(gy));
            }
            if let Some(ref fm) = c_finalmode {
                inst.set_param("mode", zennode::ParamValue::Str(fm.clone()));
            }
        }
    }

    // Inject SmartCropAnalyze before Constrain for c.focus rects.
    if let Some(CFocusParsed::Rects(ref rects)) = c_focus {
        let constrain_idx = nodes.iter().position(|n| {
            let id = n.schema().id;
            id == "zenresize.constrain" || id == "zenlayout.constrain"
        });

        if let Some(idx) = constrain_idx {
            let constrain = &nodes[idx];
            let target_w = constrain
                .get_param("w")
                .and_then(|v| v.as_u32())
                .unwrap_or(0);
            let target_h = constrain
                .get_param("h")
                .and_then(|v| v.as_u32())
                .unwrap_or(0);

            if target_w > 0 && target_h > 0 {
                let csv: String = rects
                    .iter()
                    .flat_map(|r| r.iter().copied())
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(",");

                let mut analyze = crate::zennode_defs::SmartCropAnalyze::default();
                analyze.rects_csv = csv;
                analyze.target_w = target_w;
                analyze.target_h = target_h;
                analyze.zoom = c_zoom.unwrap_or(false);

                nodes.insert(idx, Box::new(analyze));
            }
        }
    }

    // Faces / Auto: silently ignored without nodes-faces feature.
    #[cfg(feature = "nodes-faces")]
    if matches!(c_focus, Some(CFocusParsed::Faces | CFocusParsed::Auto)) {
        // TODO: Build ML Analyze closure when nodes-faces is available.
    }
}

/// Parse `c.focus` from a raw querystring.
///
/// Supports:
/// - `c.focus=faces` → `Faces`
/// - `c.focus=auto` → `Auto`
/// - `c.focus=50,30` → `Point(50.0, 30.0)`
/// - `c.focus=20,30,80,90` → `Rects([[20,30,80,90]])`
/// - `c.focus=20,30,80,90,10,10,40,40` → `Rects([[20,30,80,90],[10,10,40,40]])`
///
/// Returns `None` on parse failure (graceful degradation, no crash).
fn parse_c_focus(querystring: &str) -> Option<CFocusParsed> {
    for part in querystring.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if !key.eq_ignore_ascii_case("c.focus") {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return None;
        }

        // Check keywords first.
        if value.eq_ignore_ascii_case("faces") {
            return Some(CFocusParsed::Faces);
        }
        if value.eq_ignore_ascii_case("auto") {
            return Some(CFocusParsed::Auto);
        }

        // Parse as comma-separated floats.
        let floats: Vec<f32> = value
            .split(',')
            .map(|s| s.trim().parse::<f32>())
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        match floats.len() {
            2 => return Some(CFocusParsed::Point(floats[0], floats[1])),
            n if n >= 4 && n % 4 == 0 => {
                let rects: Vec<[f32; 4]> = floats
                    .chunks_exact(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect();
                return Some(CFocusParsed::Rects(rects));
            }
            _ => return None, // Invalid count — silent failure
        }
    }
    None
}

/// Parse `c.zoom=true|false` from a raw querystring.
fn parse_c_zoom(querystring: &str) -> Option<bool> {
    for part in querystring.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("c.zoom") {
            return match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            };
        }
    }
    None
}

/// Parse `c.finalmode` from a raw querystring.
fn parse_c_finalmode(querystring: &str) -> Option<String> {
    for part in querystring.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("c.finalmode") {
            let v = value.trim();
            if v.is_empty() {
                return None;
            }
            return Some(v.to_string());
        }
    }
    None
}
