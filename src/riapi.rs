//! RIAPI querystring preprocessing and IR4 phase ordering.
//!
//! Two shared building blocks used by every querystring entry point
//! (`imageflow_compat::riapi::expand_zen` and the `zenpipe` CLI `--qs` path):
//!
//! 1. [`preprocess_querystring`] — pure string→string normalization that runs
//!    BEFORE the zennode registry parse: `srcset=`/`short=` expansion, legacy
//!    IR4 shortcut pairs (`stretch=fill`, `crop=auto`, `mode=carve`), value
//!    coercions the typed params can't express (`s.sepia=true`,
//!    `s.grayscale=true`, trailing-`x` zoom values), same-axis
//!    `width`/`maxwidth` reconciliation, and the IR4 default-mode rule
//!    (`width`/`height` present → `mode=pad`; only `maxwidth`/`maxheight` →
//!    `mode=max`).
//! 2. [`riapi_order`] — stable bucket sort of parsed node instances into the
//!    IR4 emission order (trim → source rotate/flip → crop → constrain →
//!    corners → filters → white balance → overlays → padding → post
//!    rotate/flip). Querystring keys have no order, so the parser output is
//!    registration-order noise; this puts it in the order ImageResizer 4 and
//!    imageflow apply operations.
//!
//! Reference behavior: `imageflow_riapi/src/ir4/{parsing,layout}.rs`
//! (`Ir4Layout::add_steps` for the emission order, `FitModeStrings::None`
//! docs for the default-mode rule).

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One normalized querystring pair.
#[derive(Clone, Debug)]
struct Pair {
    /// Lowercased key.
    key: String,
    value: String,
}

fn parse_pairs(qs: &str) -> Vec<Pair> {
    qs.trim_start_matches('?')
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => Pair {
                key: k.to_ascii_lowercase(),
                value: v.to_owned(),
            },
            None => Pair {
                key: p.to_ascii_lowercase(),
                value: String::new(),
            },
        })
        .collect()
}

fn to_querystring(pairs: &[Pair]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .map(|p| format!("{}={}", p.key, p.value))
        .collect();
    parts.join("&")
}

fn is_true(v: &str) -> bool {
    v.eq_ignore_ascii_case("true")
        || v == "1"
        || v.eq_ignore_ascii_case("yes")
        || v.eq_ignore_ascii_case("on")
}

fn is_false(v: &str) -> bool {
    v.eq_ignore_ascii_case("false")
        || v == "0"
        || v.eq_ignore_ascii_case("no")
        || v.eq_ignore_ascii_case("off")
}

/// Normalize a RIAPI querystring before registry parsing.
///
/// Returns the rewritten querystring plus human-readable warnings for
/// value rewrites that changed semantics (`mode=carve`).
///
/// This is intentionally a pure string transform: it neither knows about
/// nodes nor about source dimensions. Anything requiring dimensions
/// (cross-axis `width`+`maxheight` bounding, crop-unit resolution) happens
/// later, at the geometry bridge where dimensions exist.
pub fn preprocess_querystring(qs: &str) -> (String, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();

    // 1. srcset=/short= micro-syntax expands into plain pairs first, so the
    //    passes below (and the registry) see the expanded form.
    let expanded = crate::srcset::expand_srcset(qs);
    let mut pairs = parse_pairs(&expanded);

    // 2. Legacy shortcut pairs and value rewrites.
    let mut forced_mode: Option<&'static str> = None;
    for p in &mut pairs {
        match p.key.as_str() {
            // `stretch=fill` → mode=stretch (overrides an explicit mode=, per IR4).
            "stretch" if p.value.eq_ignore_ascii_case("fill") => {
                forced_mode = Some("stretch");
                p.key.clear(); // mark for removal
            }
            // `crop=auto` → mode=crop (the crop *coordinates* key is untouched).
            "crop" if p.value.eq_ignore_ascii_case("auto") => {
                forced_mode = Some("crop");
                p.key.clear();
            }
            "mode" => {
                if p.value.eq_ignore_ascii_case("carve") {
                    warnings.push(
                        "mode=carve (seam carving) is not supported; treating as mode=stretch"
                            .to_string(),
                    );
                    p.value = "stretch".to_string();
                } else if p.value.eq_ignore_ascii_case("none") {
                    // mode=none = "decide from other keys" — same as absent.
                    p.key.clear();
                }
            }
            // Typed f32 param — IR4 accepted booleans here.
            "s.sepia" => {
                if is_true(&p.value) {
                    p.value = "1".to_string();
                } else if is_false(&p.value) {
                    p.value = "0".to_string();
                }
            }
            // `true`/`y` are IR4 aliases for NTSC grayscale weights.
            "s.grayscale" => {
                if is_true(&p.value) || p.value.eq_ignore_ascii_case("y") {
                    p.value = "ntsc".to_string();
                }
            }
            // Trailing-`x` DPR forms: `dpr=2x`, `zoom=1.5x`.
            "zoom" | "dpr" | "dppx" | "qp.dpr" | "qp.dppx" => {
                if let Some(stripped) = p
                    .value
                    .strip_suffix('x')
                    .or_else(|| p.value.strip_suffix('X'))
                {
                    if stripped.parse::<f32>().is_ok() {
                        p.value = stripped.to_string();
                    }
                }
            }
            _ => {}
        }
    }
    pairs.retain(|p| !p.key.is_empty());

    // 3. Same-axis max reconciliation: width+maxwidth → the smaller wins.
    //    (Cross-axis bounding needs the source aspect ratio — bridge's job.)
    reconcile_axis(&mut pairs, &["w", "width"], "maxwidth");
    reconcile_axis(&mut pairs, &["h", "height"], "maxheight");

    // 4. Default fit mode (IR4 `FitModeStrings::None` rule): when no mode
    //    survives, width/height imply pad; maxwidth/maxheight alone imply max.
    let has_mode = forced_mode.is_some() || pairs.iter().any(|p| p.key == "mode");
    if !has_mode {
        let has_exact = pairs
            .iter()
            .any(|p| matches!(p.key.as_str(), "w" | "width" | "h" | "height"));
        let has_max = pairs
            .iter()
            .any(|p| matches!(p.key.as_str(), "maxwidth" | "maxheight"));
        if has_exact {
            forced_mode = Some("pad");
        } else if has_max {
            forced_mode = Some("max");
        }
    }
    if let Some(m) = forced_mode {
        pairs.retain(|p| p.key != "mode");
        pairs.push(Pair {
            key: "mode".to_string(),
            value: m.to_string(),
        });
    }

    (to_querystring(&pairs), warnings)
}

/// If both an exact key and its legacy max key are present on one axis,
/// keep the smaller value under the exact key and drop the max key.
fn reconcile_axis(pairs: &mut Vec<Pair>, exact_keys: &[&str], max_key: &str) {
    let max_val: Option<u32> = pairs
        .iter()
        .find(|p| p.key == max_key)
        .and_then(|p| p.value.trim().parse::<u32>().ok());
    let Some(max_val) = max_val else { return };
    let exact_val: Option<u32> = pairs
        .iter()
        .find(|p| exact_keys.contains(&p.key.as_str()))
        .and_then(|p| p.value.trim().parse::<u32>().ok());
    let Some(exact_val) = exact_val else { return };

    let keep = exact_val.min(max_val);
    for p in pairs.iter_mut() {
        if exact_keys.contains(&p.key.as_str()) {
            p.value = keep.to_string();
        }
    }
    pairs.retain(|p| p.key != max_key);
}

/// Stable IR4 phase bucket for a parsed node instance, by schema id.
///
/// Buckets follow `Ir4Layout::add_steps` (imageflow_riapi layout.rs:473-647):
/// decode config → EXIF orient → whitespace trim → source rotate/flip →
/// crop → smart-crop analysis → constrain/resize → round corners → color
/// filters → white balance → overlays → canvas padding → post rotate →
/// post flip → quantize → everything else.
///
/// In querystring context every plain `rotate_90/180/270`, `flip_h`, and
/// `flip_v` instance comes from `srotate=`/`sflip=` (source ops); the
/// post-resize `rotate=`/`flip=` keys produce the dedicated
/// `zenpipe.post_rotate` / `zenpipe.post_flip` nodes.
#[cfg(feature = "zennode")]
fn riapi_bucket(schema_id: &str) -> u32 {
    if schema_id.starts_with("zenfilters.") {
        return 60;
    }
    match schema_id {
        // Decode-side configuration.
        "zennode.decode" | "zenjpeg.decode" | "zenwebp.decode" | "zenjxl.decode"
        | "heic.decode" | "zenpipe.riapi.frame" | "zenpipe.riapi.icc" | "zenpipe.riapi.hdr" => 0,
        "zenlayout.orient" => 10,
        "zenpipe.crop_whitespace" => 15,
        // Source rotate/flip (srotate=/sflip=), pre-crop.
        "zenlayout.rotate_90" | "zenlayout.rotate_180" | "zenlayout.rotate_270"
        | "zenlayout.flip_h" | "zenlayout.flip_v" => 20,
        "zenlayout.crop" | "zenlayout.crop_percent" | "zenlayout.crop_margins"
        | "zenpipe.riapi_crop" => 30,
        "zenpipe.smart_crop_analyze" => 35,
        "zenresize.constrain" | "zenlayout.constrain" | "zenresize.resize" => 40,
        "zenpipe.round_corners" => 50,
        // (zenfilters.* handled above = 60)
        "imageflow.white_balance_srgb" | "imageflow.color_matrix_srgb" => 65,
        "zenpipe.composite" | "zenpipe.overlay" => 70,
        "zenlayout.expand_canvas" => 75,
        "zenpipe.post_rotate" => 80,
        "zenpipe.post_flip" => 85,
        "zenquant.quantize" => 90,
        _ => 95,
    }
}

/// Sort parsed querystring nodes into IR4 emission order (stable).
///
/// Call this on the output of `NodeRegistry::from_querystring` before
/// handing nodes to the bridge. JSON- and Rust-built pipelines must NOT be
/// passed through this — declaration order is the contract there.
#[cfg(feature = "zennode")]
pub fn riapi_order(nodes: &mut [alloc::boxed::Box<dyn zennode::NodeInstance>]) {
    nodes.sort_by_key(|n| riapi_bucket(n.schema().id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qs_map(qs: &str) -> Vec<(String, String)> {
        parse_pairs(qs)
            .into_iter()
            .map(|p| (p.key, p.value))
            .collect()
    }

    #[test]
    fn default_mode_pad_for_exact_dims() {
        let (out, w) = preprocess_querystring("w=800&h=600");
        assert!(w.is_empty());
        assert!(qs_map(&out).contains(&("mode".into(), "pad".into())));
    }

    #[test]
    fn default_mode_max_for_legacy_max_dims() {
        let (out, _) = preprocess_querystring("maxwidth=800");
        assert!(qs_map(&out).contains(&("mode".into(), "max".into())));
    }

    #[test]
    fn no_mode_injected_without_dims() {
        let (out, _) = preprocess_querystring("s.sepia=1");
        assert!(!out.contains("mode="));
    }

    #[test]
    fn explicit_mode_wins() {
        let (out, _) = preprocess_querystring("w=800&h=600&mode=crop");
        let m = qs_map(&out);
        assert_eq!(m.iter().filter(|(k, _)| k == "mode").count(), 1);
        assert!(m.contains(&("mode".into(), "crop".into())));
    }

    #[test]
    fn stretch_fill_forces_stretch_mode() {
        let (out, _) = preprocess_querystring("w=10&h=10&mode=max&stretch=fill");
        let m = qs_map(&out);
        assert!(m.contains(&("mode".into(), "stretch".into())));
        assert!(!m.iter().any(|(k, _)| k == "stretch"));
    }

    #[test]
    fn crop_auto_forces_crop_mode_and_keeps_coordinate_crop_key_absent() {
        let (out, _) = preprocess_querystring("w=10&h=10&crop=auto");
        let m = qs_map(&out);
        assert!(m.contains(&("mode".into(), "crop".into())));
        assert!(!m.iter().any(|(k, _)| k == "crop"));
    }

    #[test]
    fn crop_coordinates_pass_through() {
        let (out, _) = preprocess_querystring("crop=10,20,300,400");
        assert!(qs_map(&out).contains(&("crop".into(), "10,20,300,400".into())));
    }

    #[test]
    fn carve_becomes_stretch_with_warning() {
        let (out, w) = preprocess_querystring("w=5&mode=carve");
        assert!(qs_map(&out).contains(&("mode".into(), "stretch".into())));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn mode_none_behaves_like_absent() {
        let (out, _) = preprocess_querystring("w=800&mode=none");
        assert!(qs_map(&out).contains(&("mode".into(), "pad".into())));
    }

    #[test]
    fn sepia_bool_coerced() {
        let (out, _) = preprocess_querystring("s.sepia=true");
        assert!(qs_map(&out).contains(&("s.sepia".into(), "1".into())));
    }

    #[test]
    fn grayscale_true_and_y_coerced_to_ntsc() {
        let (a, _) = preprocess_querystring("s.grayscale=true");
        assert!(qs_map(&a).contains(&("s.grayscale".into(), "ntsc".into())));
        let (b, _) = preprocess_querystring("s.grayscale=y");
        assert!(qs_map(&b).contains(&("s.grayscale".into(), "ntsc".into())));
    }

    #[test]
    fn dpr_trailing_x_stripped() {
        let (out, _) = preprocess_querystring("dpr=2x");
        assert!(qs_map(&out).contains(&("dpr".into(), "2".into())));
    }

    #[test]
    fn same_axis_max_reconciled_smaller_wins() {
        let (out, _) = preprocess_querystring("width=100&maxwidth=50");
        let m = qs_map(&out);
        assert!(m.contains(&("width".into(), "50".into())));
        assert!(!m.iter().any(|(k, _)| k == "maxwidth"));

        let (out2, _) = preprocess_querystring("w=40&maxwidth=90");
        assert!(qs_map(&out2).contains(&("w".into(), "40".into())));
    }

    #[test]
    fn srcset_expansion_is_wired() {
        // srcset.rs expands `srcset=300w` into w=300 + mode/scale pairs.
        let (out, _) = preprocess_querystring("srcset=300w");
        assert!(out.contains("w=300"), "srcset must expand: got {out}");
    }
}
