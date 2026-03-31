//! Query string tokenizer and value parsers.
//!
//! Minimal percent-decoding and key-value extraction without external dependencies.

use alloc::string::String;
use alloc::vec::Vec;

use crate::float_math::F64Ext;

use super::ParseWarning;
use super::color::parse_color;
use super::instructions::{Anchor1D, CFocus, FitMode, Instructions, ScaleMode};

/// Known non-layout keys that should be preserved in `extras` without warnings.
/// Sorted for binary search.
const KNOWN_EXTRAS: &[&str] = &[
    "a.balancewhite",
    "a.blur",
    "a.removenoise",
    "a.sharpen",
    "accept.avif",
    "accept.color_profiles",
    "accept.jxl",
    "accept.webp",
    "avif.quality",
    "avif.speed",
    "builder",
    "cache",
    "decoder",
    "dither",
    "down.colorspace",
    "down.filter",
    "encoder",
    "f.sharpen",
    "f.sharpen_when",
    "floatspace",
    "format",
    "frame",
    "ignore_icc_errors",
    "ignoreicc",
    "jpeg.chroma",
    "jpeg.progressive",
    "jpeg.quality",
    "jxl.quality",
    "jxl.speed",
    "lossless",
    "page",
    "png.colors",
    "png.quality",
    "preset",
    "process",
    "qp",
    "qp.dpr",
    "quality",
    "s.alpha",
    "s.brightness",
    "s.contrast",
    "s.grayscale",
    "s.roundcorners",
    "s.saturation",
    "s.sepia",
    "subsampling",
    "trim.percentpadding",
    "trim.threshold",
    "up.colorspace",
    "up.filter",
    "watermark",
    "watermark_red_dot",
    "webp.lossless",
    "webp.quality",
];

/// Parse a RIAPI query string into Instructions + warnings.
pub(crate) fn parse_query(query: &str) -> (Instructions, Vec<ParseWarning>) {
    let mut inst = Instructions::new();
    let mut warnings = Vec::new();

    for pair in split_query(query) {
        let (raw_key, raw_value) = split_pair(pair);
        let key = percent_decode(&raw_key);
        let value = percent_decode(&raw_value);
        let key_lower = ascii_lowercase(&key);

        dispatch_key(&key_lower, &value, &mut inst, &mut warnings);
    }

    (inst, warnings)
}

fn dispatch_key(key: &str, value: &str, inst: &mut Instructions, warnings: &mut Vec<ParseWarning>) {
    match key {
        // Dimensions
        "w" | "width" => set_or_warn(&mut inst.w, parse_i32(value), key, value, warnings),
        "h" | "height" => set_or_warn(&mut inst.h, parse_i32(value), key, value, warnings),
        "maxwidth" => {
            set_or_warn(
                &mut inst.legacy_max_width,
                parse_i32(value),
                key,
                value,
                warnings,
            );
        }
        "maxheight" => {
            set_or_warn(
                &mut inst.legacy_max_height,
                parse_i32(value),
                key,
                value,
                warnings,
            );
        }

        // Zoom / DPR
        "zoom" | "dpr" | "dppx" => {
            set_or_warn(&mut inst.zoom, parse_dpr(value), key, value, warnings);
        }

        // Mode
        "mode" => {
            if let Some(m) = parse_fit_mode(value) {
                set_or_warn(&mut inst.mode, Some(m), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: leak_key(key),
                    value: String::from(value),
                    reason: "expected max|pad|crop|stretch|aspectcrop",
                });
            }
        }

        // Legacy mode shortcuts
        "stretch" => {
            if value.eq_ignore_ascii_case("fill") && inst.mode.is_none() {
                inst.mode = Some(FitMode::Stretch);
            }
        }
        "crop" if value.eq_ignore_ascii_case("auto") => {
            if inst.mode.is_none() {
                inst.mode = Some(FitMode::Crop);
            }
        }

        // Scale
        "scale" => {
            if let Some(s) = parse_scale_mode(value) {
                set_or_warn(&mut inst.scale, Some(s), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: leak_key(key),
                    value: String::from(value),
                    reason: "expected down|up|both|canvas",
                });
            }
        }

        // Flip
        "flip" => {
            if let Some(f) = parse_flip(value) {
                set_or_warn(&mut inst.flip, Some(f), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "flip",
                    value: String::from(value),
                    reason: "expected none|h|x|v|y|both|xy",
                });
            }
        }
        "sflip" | "sourceflip" => {
            if let Some(f) = parse_flip(value) {
                set_or_warn(&mut inst.sflip, Some(f), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: leak_key(key),
                    value: String::from(value),
                    reason: "expected none|h|x|v|y|both|xy",
                });
            }
        }

        // Rotation
        "srotate" => {
            set_or_warn(
                &mut inst.srotate,
                parse_rotation(value),
                key,
                value,
                warnings,
            );
        }
        "rotate" => {
            set_or_warn(
                &mut inst.rotate,
                parse_rotation(value),
                key,
                value,
                warnings,
            );
        }
        "autorotate" => {
            if let Some(b) = parse_bool(value) {
                set_or_warn(&mut inst.autorotate, Some(b), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "autorotate",
                    value: String::from(value),
                    reason: "expected true|false|1|0|yes|no|on|off",
                });
            }
        }

        // Crop (strict `c=` sets cropxunits=cropyunits=100)
        "c" => {
            if let Some(vals) = parse_crop_strict(value) {
                inst.crop = Some(vals);
                inst.cropxunits = Some(100.0);
                inst.cropyunits = Some(100.0);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "c",
                    value: String::from(value),
                    reason: "expected x1,y1,x2,y2 (4 numbers)",
                });
            }
        }
        // `crop=` parameter: lenient parsing (strips parens)
        "crop" => {
            // Skip if already handled as mode shortcut ("crop=auto")
            if !value.eq_ignore_ascii_case("auto") {
                if let Some(vals) = parse_crop_lenient(value) {
                    set_or_warn(&mut inst.crop, Some(vals), key, value, warnings);
                } else {
                    warnings.push(ParseWarning::ValueInvalid {
                        key: "crop",
                        value: String::from(value),
                        reason: "expected (x1,y1,x2,y2) or x1,y1,x2,y2",
                    });
                }
            }
        }
        "cropxunits" => {
            set_or_warn(&mut inst.cropxunits, parse_f64(value), key, value, warnings);
        }
        "cropyunits" => {
            set_or_warn(&mut inst.cropyunits, parse_f64(value), key, value, warnings);
        }

        // Anchor / gravity
        "anchor" => {
            if let Some(a) = parse_anchor(value) {
                set_or_warn(&mut inst.anchor, Some(a), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "anchor",
                    value: String::from(value),
                    reason: "expected position name or x,y percentages",
                });
            }
        }
        "c.gravity" => {
            if let Some(g) = parse_gravity(value) {
                set_or_warn(&mut inst.c_gravity, Some(g), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "c.gravity",
                    value: String::from(value),
                    reason: "expected x,y percentages (0-100)",
                });
            }
        }
        "c.focus" => {
            if let Some(f) = parse_c_focus(value) {
                set_or_warn(&mut inst.c_focus, Some(f), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "c.focus",
                    value: String::from(value),
                    reason: "expected faces|auto|x,y|x1,y1,x2,y2[,...]",
                });
            }
        }
        "c.zoom" => {
            if let Some(b) = parse_bool(value) {
                set_or_warn(&mut inst.c_zoom, Some(b), key, value, warnings);
            } else {
                warnings.push(ParseWarning::ValueInvalid {
                    key: "c.zoom",
                    value: String::from(value),
                    reason: "expected true|false|1|0|yes|no|on|off",
                });
            }
        }
        "c.finalmode" => {
            set_or_warn(
                &mut inst.c_finalmode,
                Some(String::from(value)),
                key,
                value,
                warnings,
            );
        }

        // Background color
        "bgcolor" | "s.bgcolor" => {
            if let Some(c) = parse_color(value) {
                set_or_warn(&mut inst.bgcolor, Some(c), key, value, warnings);
            } else if !value.is_empty() {
                warnings.push(ParseWarning::ValueInvalid {
                    key: leak_key(key),
                    value: String::from(value),
                    reason: "expected hex color or CSS3 color name",
                });
            }
        }

        // srcset/short — Phase 2, accept without warning
        "srcset" | "short" => {
            inst.extras.insert(String::from(key), String::from(value));
        }

        // Known non-layout keys → extras, no warning
        _ => {
            if KNOWN_EXTRAS.binary_search(&key).is_ok() {
                inst.extras.insert(String::from(key), String::from(value));
            } else {
                warnings.push(ParseWarning::KeyNotRecognized {
                    key: String::from(key),
                    value: String::from(value),
                });
            }
        }
    }
}

/// Set a field, warning on duplicate.
fn set_or_warn<T>(
    field: &mut Option<T>,
    parsed: Option<T>,
    key: &str,
    value: &str,
    warnings: &mut Vec<ParseWarning>,
) {
    if let Some(v) = parsed {
        if field.is_some() {
            warnings.push(ParseWarning::DuplicateKey {
                key: String::from(key),
                value: String::from(value),
            });
        }
        *field = Some(v);
    }
}

// ---- Value parsers ----

fn parse_i32(s: &str) -> Option<i32> {
    s.trim().parse::<i32>().ok().filter(|&v| v > 0)
}

fn parse_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Parse DPR/zoom value, stripping trailing "x" suffix.
fn parse_dpr(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('x').trim_end_matches('X');
    s.parse::<f64>().ok().filter(|&v| v > 0.0 && v.is_finite())
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_fit_mode(s: &str) -> Option<FitMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "max" => Some(FitMode::Max),
        "pad" => Some(FitMode::Pad),
        "crop" => Some(FitMode::Crop),
        "stretch" | "carve" => Some(FitMode::Stretch),
        "aspectcrop" => Some(FitMode::AspectCrop),
        _ => None,
    }
}

fn parse_scale_mode(s: &str) -> Option<ScaleMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "down" | "downscaleonly" => Some(ScaleMode::DownscaleOnly),
        "up" | "upscaleonly" => Some(ScaleMode::UpscaleOnly),
        "both" => Some(ScaleMode::Both),
        "canvas" | "upscalecanvas" => Some(ScaleMode::UpscaleCanvas),
        _ => None,
    }
}

fn parse_flip(s: &str) -> Option<(bool, bool)> {
    match s.trim().to_ascii_lowercase().as_str() {
        "none" | "" => Some((false, false)),
        "h" | "x" => Some((true, false)),
        "v" | "y" => Some((false, true)),
        "both" | "xy" => Some((true, true)),
        _ => None,
    }
}

/// Normalize rotation to 0/90/180/270. Rounds to nearest 90, mod 360.
fn parse_rotation(s: &str) -> Option<i32> {
    let v: f64 = s.trim().parse().ok()?;
    let quarters = (v / 90.0).round_() as i32;
    let normalized = ((quarters % 4) + 4) % 4 * 90;
    Some(normalized)
}

/// Named anchor positions.
fn parse_anchor(s: &str) -> Option<(Anchor1D, Anchor1D)> {
    match s.trim().to_ascii_lowercase().as_str() {
        "topleft" => Some((Anchor1D::Near, Anchor1D::Near)),
        "topcenter" => Some((Anchor1D::Center, Anchor1D::Near)),
        "topright" => Some((Anchor1D::Far, Anchor1D::Near)),
        "middleleft" => Some((Anchor1D::Near, Anchor1D::Center)),
        "middlecenter" => Some((Anchor1D::Center, Anchor1D::Center)),
        "middleright" => Some((Anchor1D::Far, Anchor1D::Center)),
        "bottomleft" => Some((Anchor1D::Near, Anchor1D::Far)),
        "bottomcenter" => Some((Anchor1D::Center, Anchor1D::Far)),
        "bottomright" => Some((Anchor1D::Far, Anchor1D::Far)),
        other => {
            // Numeric: "50,25" → (Percent(50.0), Percent(25.0))
            let parts: Vec<&str> = other.split(',').collect();
            if parts.len() == 2 {
                let x: f32 = parts[0].trim().parse().ok()?;
                let y: f32 = parts[1].trim().parse().ok()?;
                Some((Anchor1D::Percent(x), Anchor1D::Percent(y)))
            } else {
                None
            }
        }
    }
}

/// Parse `c.gravity` as two f64 values [x%, y%].
fn parse_gravity(s: &str) -> Option<[f64; 2]> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 2 {
        let x = parse_f64(parts[0])?;
        let y = parse_f64(parts[1])?;
        Some([x, y])
    } else {
        None
    }
}

/// Parse `c.focus` value: keywords, 2-value point, or groups-of-4 rects.
///
/// Supports both semicolon-separated rects (`20,30,80,90;10,10,40,40`)
/// and flat comma groups (`20,30,80,90,10,10,40,40`) for compatibility
/// with both ImageResizer and the focus-rects-smart-crop branch.
fn parse_c_focus(s: &str) -> Option<CFocus> {
    let trimmed = s.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "faces" => return Some(CFocus::Faces),
        "saliency" => return Some(CFocus::Saliency),
        "auto" => return Some(CFocus::Auto),
        _ => {}
    }

    // Semicolon-separated rects: "20,30,80,90;10,10,40,40"
    if trimmed.contains(';') {
        let mut rects = Vec::new();
        for group in trimmed.split(';') {
            let group = group.trim();
            if group.is_empty() {
                continue;
            }
            let values: Vec<f64> = group
                .split(',')
                .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
                .collect::<Option<Vec<f64>>>()?;
            if values.len() != 4 {
                return None;
            }
            rects.push([values[0], values[1], values[2], values[3]]);
        }
        return if rects.is_empty() {
            None
        } else {
            Some(CFocus::Rects(rects))
        };
    }

    // Flat comma-separated: 2 values = point, N*4 values = rects
    let parts: Vec<&str> = trimmed.split(',').collect();
    let values: Vec<f64> = parts
        .iter()
        .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<Vec<f64>>>()?;

    match values.len() {
        2 => Some(CFocus::Point([values[0], values[1]])),
        n if n >= 4 && n % 4 == 0 => {
            let rects = values
                .chunks_exact(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect();
            Some(CFocus::Rects(rects))
        }
        _ => None,
    }
}

/// Strict crop parsing: exactly 4 comma-separated f64 values.
fn parse_crop_strict(s: &str) -> Option<[f64; 4]> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let x1 = parse_f64(parts[0])?;
    let y1 = parse_f64(parts[1])?;
    let x2 = parse_f64(parts[2])?;
    let y2 = parse_f64(parts[3])?;
    Some([x1, y1, x2, y2])
}

/// Lenient crop parsing: strips parens, unparseable values become 0.
fn parse_crop_lenient(s: &str) -> Option<[f64; 4]> {
    let s = s.trim().trim_start_matches('(').trim_end_matches(')');
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let parse_or_zero = |s: &str| -> f64 {
        s.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    };
    let vals: [f64; 4] = [
        parse_or_zero(parts[0]),
        parse_or_zero(parts[1]),
        parse_or_zero(parts[2]),
        parse_or_zero(parts[3]),
    ];
    Some(vals)
}

// ---- Query string tokenizer ----

/// Split query string on '&'.
fn split_query(query: &str) -> impl Iterator<Item = &str> {
    // Strip leading '?' if present (caller may or may not have stripped it)
    let query = query.strip_prefix('?').unwrap_or(query);
    query.split('&').filter(|s| !s.is_empty())
}

/// Split a single "key=value" pair on the first '='.
fn split_pair(pair: &str) -> (String, String) {
    match pair.find('=') {
        Some(pos) => (String::from(&pair[..pos]), String::from(&pair[pos + 1..])),
        None => (String::from(pair), String::new()),
    }
}

/// Percent-decode a URL component. Also handles '+' as space.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                result.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                    result.push((hi << 4 | lo) as char);
                    i += 3;
                } else {
                    result.push('%');
                    i += 1;
                }
            }
            ch => {
                result.push(ch as char);
                i += 1;
            }
        }
    }
    result
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn ascii_lowercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// Return a `&'static str` for known key names used in warnings.
/// For truly dynamic keys, this leaks — but parse warnings are rare.
fn leak_key(key: &str) -> &'static str {
    match key {
        "w" | "width" => "w",
        "h" | "height" => "h",
        "maxwidth" => "maxwidth",
        "maxheight" => "maxheight",
        "zoom" | "dpr" | "dppx" => "zoom",
        "mode" => "mode",
        "scale" => "scale",
        "flip" => "flip",
        "sflip" => "sflip",
        "sourceflip" => "sflip",
        "srotate" => "srotate",
        "rotate" => "rotate",
        "autorotate" => "autorotate",
        "c" => "c",
        "crop" => "crop",
        "cropxunits" => "cropxunits",
        "cropyunits" => "cropyunits",
        "anchor" => "anchor",
        "c.gravity" => "c.gravity",
        "c.focus" => "c.focus",
        "c.zoom" => "c.zoom",
        "c.finalmode" => "c.finalmode",
        "bgcolor" => "bgcolor",
        "s.bgcolor" => "bgcolor",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extras_is_sorted() {
        for w in KNOWN_EXTRAS.windows(2) {
            assert!(
                w[0] < w[1],
                "KNOWN_EXTRAS not sorted: {:?} >= {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn parse_basic_dimensions() {
        let (inst, warnings) = parse_query("w=800&h=600");
        assert_eq!(inst.w, Some(800));
        assert_eq!(inst.h, Some(600));
        assert!(warnings.is_empty());
    }

    #[test]
    fn parse_width_height_aliases() {
        let (inst, _) = parse_query("width=800&height=600");
        assert_eq!(inst.w, Some(800));
        assert_eq!(inst.h, Some(600));
    }

    #[test]
    fn parse_mode_and_scale() {
        let (inst, _) = parse_query("w=800&h=600&mode=crop&scale=both");
        assert_eq!(inst.mode, Some(FitMode::Crop));
        assert_eq!(inst.scale, Some(ScaleMode::Both));
    }

    #[test]
    fn parse_mode_case_insensitive() {
        let (inst, _) = parse_query("mode=AspectCrop");
        assert_eq!(inst.mode, Some(FitMode::AspectCrop));
    }

    #[test]
    fn parse_stretch_fill_shortcut() {
        let (inst, _) = parse_query("w=800&h=600&stretch=fill");
        assert_eq!(inst.mode, Some(FitMode::Stretch));
    }

    #[test]
    fn parse_crop_auto_shortcut() {
        let (inst, _) = parse_query("w=800&h=600&crop=auto");
        assert_eq!(inst.mode, Some(FitMode::Crop));
    }

    #[test]
    fn parse_zoom_dpr() {
        let (inst, _) = parse_query("w=400&dpr=2x");
        assert_eq!(inst.zoom, Some(2.0));

        let (inst, _) = parse_query("w=400&zoom=1.5");
        assert_eq!(inst.zoom, Some(1.5));
    }

    #[test]
    fn parse_rotation() {
        let (inst, _) = parse_query("srotate=90&rotate=270");
        assert_eq!(inst.srotate, Some(90));
        assert_eq!(inst.rotate, Some(270));
    }

    #[test]
    fn rotation_rounds_to_90() {
        let (inst, _) = parse_query("rotate=95");
        assert_eq!(inst.rotate, Some(90));

        let (inst, _) = parse_query("rotate=-90");
        assert_eq!(inst.rotate, Some(270));
    }

    #[test]
    fn parse_flip() {
        let (inst, _) = parse_query("flip=h&sflip=both");
        assert_eq!(inst.flip, Some((true, false)));
        assert_eq!(inst.sflip, Some((true, true)));
    }

    #[test]
    fn parse_autorotate() {
        let (inst, _) = parse_query("autorotate=false");
        assert_eq!(inst.autorotate, Some(false));

        let (inst, _) = parse_query("autorotate=1");
        assert_eq!(inst.autorotate, Some(true));
    }

    #[test]
    fn parse_c_crop_percent() {
        let (inst, _) = parse_query("c=10,10,90,90");
        assert_eq!(inst.crop, Some([10.0, 10.0, 90.0, 90.0]));
        assert_eq!(inst.cropxunits, Some(100.0));
        assert_eq!(inst.cropyunits, Some(100.0));
    }

    #[test]
    fn parse_crop_with_units() {
        let (inst, _) = parse_query("crop=100,100,900,900&cropxunits=1000&cropyunits=1000");
        assert_eq!(inst.crop, Some([100.0, 100.0, 900.0, 900.0]));
        assert_eq!(inst.cropxunits, Some(1000.0));
        assert_eq!(inst.cropyunits, Some(1000.0));
    }

    #[test]
    fn parse_crop_lenient_parens() {
        let (inst, _) = parse_query("crop=(10,20,30,40)");
        assert_eq!(inst.crop, Some([10.0, 20.0, 30.0, 40.0]));
    }

    #[test]
    fn parse_anchor_named() {
        let (inst, _) = parse_query("anchor=topleft");
        assert_eq!(inst.anchor, Some((Anchor1D::Near, Anchor1D::Near)));

        let (inst, _) = parse_query("anchor=bottomright");
        assert_eq!(inst.anchor, Some((Anchor1D::Far, Anchor1D::Far)));
    }

    #[test]
    fn parse_anchor_numeric() {
        let (inst, _) = parse_query("anchor=25,75");
        assert_eq!(
            inst.anchor,
            Some((Anchor1D::Percent(25.0), Anchor1D::Percent(75.0)))
        );
    }

    #[test]
    fn parse_c_gravity() {
        let (inst, _) = parse_query("c.gravity=30,70");
        assert_eq!(inst.c_gravity, Some([30.0, 70.0]));
    }

    #[test]
    fn parse_bgcolor_hex() {
        let (inst, _) = parse_query("bgcolor=ff0000");
        assert_eq!(
            inst.bgcolor,
            Some(crate::CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn parse_bgcolor_named() {
        let (inst, _) = parse_query("bgcolor=white");
        assert_eq!(
            inst.bgcolor,
            Some(crate::CanvasColor::Srgb {
                r: 255,
                g: 255,
                b: 255,
                a: 255
            })
        );
    }

    #[test]
    fn known_extras_preserved() {
        let (inst, warnings) = parse_query("w=800&format=webp&quality=80");
        assert_eq!(inst.extras.get("format").map(String::as_str), Some("webp"));
        assert_eq!(inst.extras.get("quality").map(String::as_str), Some("80"));
        // No warnings for known extras
        assert!(
            warnings
                .iter()
                .all(|w| !matches!(w, ParseWarning::KeyNotRecognized { .. })),
            "should not warn about known extras: {warnings:?}"
        );
    }

    #[test]
    fn unknown_key_warns() {
        let (_, warnings) = parse_query("w=800&foobar=baz");
        assert!(warnings.iter().any(|w| matches!(
            w,
            ParseWarning::KeyNotRecognized { key, .. } if key == "foobar"
        )));
    }

    #[test]
    fn percent_decoding_works() {
        let (inst, _) = parse_query("w=800&bgcolor=%23ff0000");
        assert_eq!(
            inst.bgcolor,
            Some(crate::CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn leading_question_mark_stripped() {
        let (inst, _) = parse_query("?w=800&h=600");
        assert_eq!(inst.w, Some(800));
        assert_eq!(inst.h, Some(600));
    }

    #[test]
    fn duplicate_key_warns() {
        let (inst, warnings) = parse_query("w=800&w=400");
        // Last value wins
        assert_eq!(inst.w, Some(400));
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, ParseWarning::DuplicateKey { .. }))
        );
    }

    #[test]
    fn negative_dimensions_ignored() {
        let (inst, _) = parse_query("w=-10&h=0");
        assert_eq!(inst.w, None);
        assert_eq!(inst.h, None);
    }

    #[test]
    fn maxwidth_maxheight() {
        let (inst, _) = parse_query("maxwidth=500&maxheight=300");
        assert_eq!(inst.legacy_max_width, Some(500));
        assert_eq!(inst.legacy_max_height, Some(300));
    }
}
