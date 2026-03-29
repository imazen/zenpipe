//! Srcset/short RIAPI shorthand parser.
//!
//! The srcset format is a comma-delimited shorthand for image processing
//! querystrings, designed for use in HTML `srcset` attributes:
//!
//! ```text
//! ?srcset=300w,200h,fit-crop,webp-90,qp-high,sharp-15
//! ```
//!
//! This module provides:
//! - [`parse_srcset`] — parse a srcset value into key-value pairs
//! - [`expand_srcset`] — expand `srcset`/`short` keys in a querystring
//!
//! # Supported commands
//!
//! | Command | Example | Expanded |
//! |---------|---------|----------|
//! | Width | `300w` | `w=300` |
//! | Height | `200h` | `h=200` |
//! | Zoom/DPR | `2x` | `zoom=2` |
//! | Fit mode | `fit-crop` | `mode=crop` |
//! | Format | `webp` | `format=webp` |
//! | Format+quality | `webp-90` | `format=webp&webp.quality=90` |
//! | Format+lossless | `webp-lossless` | `format=webp&lossless=true` |
//! | Quality profile | `qp-high` | `qp=high` |
//! | Lossless | `lossless` | `lossless=true` |
//! | Upscale | `upscale` | `scale=both` |
//! | Sharpen | `sharp-15` | `f.sharpen=15` |
//! | Crop | `crop-10-10-90-90` | `c=10,10,90,90` |

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

/// Known image output formats for srcset parsing.
const FORMATS: &[&str] = &["webp", "jpeg", "jpg", "png", "avif", "jxl", "gif", "auto"];

/// Parse a srcset value string into RIAPI key-value pairs.
///
/// The srcset string is comma-separated commands, where each command
/// uses hyphens to delimit arguments:
///
/// ```
/// # use std::collections::BTreeMap;
/// # // Simulate the function for doctest
/// # fn parse_srcset(_: &str) -> BTreeMap<String, String> { BTreeMap::new() }
/// let pairs = parse_srcset("300w,200h,fit-crop,webp-90");
/// ```
///
/// Commands:
/// - `300w`, `200h`, `2x` — dimensions/zoom
/// - `fit-crop`, `fit-pad`, `fit-max`, `fit-distort` — constraint mode
/// - `webp`, `jpeg-85`, `png-lossless`, `jxl-d1.0` — format + tuning
/// - `qp-high`, `qp-dpr-2x` — quality profile
/// - `lossless`, `lossy` — global lossless toggle
/// - `upscale` — allow upscaling
/// - `sharp-15`, `sharpen-20` — unsharp mask percentage
/// - `crop-10-10-90-90` — percentage crop (x1-y1-x2-y2)
pub fn parse_srcset(value: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    if value.is_empty() {
        return map;
    }

    let lowered = value.to_ascii_lowercase();

    for command_untrimmed in lowered.split(',') {
        let command = command_untrimmed.trim();
        if command.is_empty() {
            continue;
        }

        let mut args = command.split('-');
        let Some(name) = args.next() else { continue };

        // Default mode is "within" (max) — srcset always implies constrained
        // sizing, but we only emit mode if no explicit fit command overrides.

        // --- Format commands ---
        let canonical_format = match name {
            "jpg" => Some("jpeg"),
            n if FORMATS.contains(&n) => Some(n),
            _ => None,
        };

        if let Some(fmt) = canonical_format {
            map.insert("format".into(), fmt.to_string());
            parse_format_tuning(fmt, &mut args, &mut map);
            continue;
        }

        // --- Lossless/lossy as standalone commands ---
        if name == "lossless" {
            map.insert("format".into(), "auto".into());
            map.insert("lossless".into(), "true".into());
            continue;
        }
        if name == "lossy" {
            map.insert("format".into(), "auto".into());
            map.insert("lossless".into(), "false".into());
            continue;
        }

        match name {
            // --- Quality profile ---
            "qp" => {
                if let Some(arg1) = args.next() {
                    if arg1 == "dpr" || arg1 == "dppx" {
                        // qp-dpr-2x or qp-dpr-2
                        if let Some(arg2) = args.next() {
                            let number_text = arg2.strip_suffix('x').unwrap_or(arg2);
                            if let Ok(v) = number_text.parse::<f32>() {
                                map.insert("qp.dpr".into(), format_f32(v.max(0.0)));
                            }
                        }
                    } else {
                        map.insert("qp".into(), arg1.to_string());
                    }
                }
            }

            // --- Crop ---
            "crop" => {
                let parts: Vec<&str> = args.collect();
                if parts.len() == 4 {
                    // Validate all four parse as numbers
                    let ok = parts.iter().all(|p| p.parse::<f64>().is_ok());
                    if ok {
                        // Use `c` key which forces cropxunits=100, cropyunits=100
                        map.insert(
                            "c".into(),
                            alloc::format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]),
                        );
                    }
                }
            }

            // --- Fit mode ---
            "fit" => {
                if let Some(fit) = args.next() {
                    let mode_val = match fit {
                        "pad" => "fit_pad",
                        "crop" | "cover" => "crop",
                        "max" | "scale" | "contain" => "within",
                        "distort" | "fill" => "distort",
                        _ => fit, // pass through
                    };
                    map.insert("mode".into(), mode_val.to_string());
                }
            }

            // --- Upscale ---
            "upscale" => {
                map.insert("scale".into(), "both".into());
            }

            // --- Sharpen ---
            "sharp" | "sharpen" => {
                if let Some(sharpen_val) = args.next() {
                    if let Ok(v) = sharpen_val.parse::<f32>() {
                        map.insert("f.sharpen".into(), format_f32(v));
                    }
                }
            }

            // --- Dimension suffixes: 300w, 200h, 2x ---
            other => {
                if let Some(dim) = parse_dimension(other) {
                    match dim {
                        Dimension::Width(v) => {
                            map.insert("w".into(), v.round().to_string());
                        }
                        Dimension::Height(v) => {
                            map.insert("h".into(), v.round().to_string());
                        }
                        Dimension::Zoom(v) => {
                            map.insert("zoom".into(), format_f32(v));
                        }
                    }
                }
                // Unrecognized commands are silently ignored (callers get
                // warnings from KvPairs for any unconsumed keys).
            }
        }
    }

    map
}

/// Expand `srcset` and `short` keys in a querystring into individual
/// key-value pairs, returning a new querystring.
///
/// Other keys in the querystring are preserved. If `srcset`/`short`
/// expanded keys conflict with explicit keys, the srcset expansion
/// wins (last-write semantics — srcset pairs are appended).
///
/// # Example
///
/// ```
/// # fn expand_srcset(qs: &str) -> String { String::new() }
/// let expanded = expand_srcset("srcset=300w,webp-90&other=yes");
/// // Result: "other=yes&format=webp&w=300&webp.quality=90"
/// ```
pub fn expand_srcset(qs: &str) -> String {
    let mut srcset_value: Option<&str> = None;
    let mut other_pairs: Vec<(&str, &str)> = Vec::new();

    for part in qs.split('&') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let key_lower = key.to_ascii_lowercase();
        if key_lower == "srcset" || key_lower == "short" {
            srcset_value = Some(value);
        } else {
            other_pairs.push((key, value));
        }
    }

    let Some(srcset_val) = srcset_value else {
        // No srcset/short — return original querystring unchanged.
        return qs.to_string();
    };

    let expanded = parse_srcset(srcset_val);

    // Build the output querystring: non-srcset pairs first, then expanded.
    let mut parts: Vec<String> = Vec::new();

    for (k, v) in &other_pairs {
        // Skip keys that the srcset expansion will override.
        let k_lower = k.to_ascii_lowercase();
        if expanded.contains_key(&k_lower) {
            continue;
        }
        if v.is_empty() {
            parts.push((*k).to_string());
        } else {
            parts.push(alloc::format!("{}={}", k, v));
        }
    }

    for (k, v) in &expanded {
        parts.push(alloc::format!("{}={}", k, v));
    }

    parts.join("&")
}

// ── Format tuning helpers ──

/// Parse format-specific arguments after the format name.
///
/// Handles patterns like:
/// - `webp-90` → `webp.quality=90`
/// - `webp-lossless` → `lossless=true`
/// - `jxl-d1.0` → `jxl.distance=1.0`
/// - `jxl-e7` → `jxl.effort=7`
/// - `avif-s4` → `avif.speed=4`
/// - `jpeg-progressive` → `jpeg.progressive=progressive`
/// - `jpeg-baseline` → `jpeg.progressive=baseline`
fn parse_format_tuning(
    format: &str,
    args: &mut core::str::Split<'_, char>,
    map: &mut BTreeMap<String, String>,
) {
    for arg in args {
        if arg.is_empty() {
            continue;
        }

        // Lossless/lossy/keep
        if arg == "lossless" || arg == "l" {
            set_format_lossless(format, "true", map);
            continue;
        }
        if arg == "lossy" {
            set_format_lossless(format, "false", map);
            continue;
        }
        if arg == "keep" {
            set_format_lossless(format, "keep", map);
            continue;
        }

        // JPEG progressive/baseline
        if format == "jpeg" {
            if arg == "progressive" {
                map.insert("jpeg.progressive".into(), "progressive".into());
                continue;
            }
            if arg == "baseline" {
                map.insert("jpeg.progressive".into(), "baseline".into());
                continue;
            }
        }

        // Prefixed values: d (distance), s (speed), e (effort), q (quality), mq (min quality)
        if arg.starts_with("mq") {
            if let Ok(v) = arg[2..].parse::<f32>() {
                if format == "png" {
                    map.insert("png.min_quality".into(), format_f32(v.max(0.0).min(100.0)));
                }
                continue;
            }
        }

        if let Some(first_char) = arg.chars().next() {
            let rest = &arg[first_char.len_utf8()..];
            match first_char {
                'd' if !rest.is_empty() => {
                    if let Ok(v) = rest.parse::<f32>() {
                        if format == "jxl" {
                            map.insert("jxl.distance".into(), format_f32(v));
                        }
                        continue;
                    }
                }
                'e' if !rest.is_empty() => {
                    if let Ok(v) = rest.parse::<f32>() {
                        if format == "jxl" {
                            map.insert(
                                "jxl.effort".into(),
                                alloc::format!("{}", v.max(0.0).min(255.0) as u8),
                            );
                        }
                        continue;
                    }
                }
                's' if !rest.is_empty() => {
                    if let Ok(v) = rest.parse::<f32>() {
                        if format == "avif" {
                            map.insert(
                                "avif.speed".into(),
                                alloc::format!("{}", v.max(0.0).min(255.0) as u8),
                            );
                        }
                        continue;
                    }
                }
                'q' if !rest.is_empty() => {
                    if let Ok(v) = rest.parse::<f32>() {
                        set_format_quality(format, v, map);
                        continue;
                    }
                }
                _ => {}
            }
        }

        // Plain number → quality
        if let Ok(v) = arg.parse::<f32>() {
            set_format_quality(format, v, map);
            continue;
        }

        // Unrecognized format argument — silently skip.
    }
}

/// Set the appropriate format-specific quality key.
fn set_format_quality(format: &str, quality: f32, map: &mut BTreeMap<String, String>) {
    let key = match format {
        "webp" => "webp.quality",
        "jpeg" => "jpeg.quality",
        "png" => "png.quality",
        "avif" => "avif.quality",
        "jxl" => "jxl.quality",
        _ => return,
    };
    map.insert(key.into(), format_f32(quality));
}

/// Set the format-specific lossless key (or global lossless for auto).
fn set_format_lossless(format: &str, value: &str, map: &mut BTreeMap<String, String>) {
    match format {
        "webp" => {
            map.insert("lossless".into(), value.into());
        }
        "png" => {
            map.insert("png.lossless".into(), value.into());
        }
        "jxl" => {
            map.insert("jxl.lossless".into(), value.into());
        }
        "auto" => {
            map.insert("lossless".into(), value.into());
        }
        // JPEG, GIF, AVIF don't have lossless keys (or handled differently)
        _ => {
            map.insert("lossless".into(), value.into());
        }
    }
}

// ── Dimension parsing ──

enum Dimension {
    Width(f32),
    Height(f32),
    Zoom(f32),
}

/// Parse a dimension token like `300w`, `200h`, or `2x`.
fn parse_dimension(token: &str) -> Option<Dimension> {
    if token.len() < 2 {
        return None;
    }
    let suffix = token.as_bytes()[token.len() - 1];
    let number_part = &token[..token.len() - 1];

    match suffix {
        b'w' => number_part.parse::<f32>().ok().map(Dimension::Width),
        b'h' => number_part.parse::<f32>().ok().map(Dimension::Height),
        b'x' => number_part.parse::<f32>().ok().map(Dimension::Zoom),
        _ => None,
    }
}

/// Format an f32 as a clean string (no trailing zeros for integers).
fn format_f32(v: f32) -> String {
    if v == v.trunc() && v.abs() < 1e9 {
        alloc::format!("{}", v as i64)
    } else {
        alloc::format!("{}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_srcset() {
        let map = parse_srcset("");
        assert!(map.is_empty());
    }

    #[test]
    fn width_only() {
        let map = parse_srcset("300w");
        assert_eq!(map.get("w").map(String::as_str), Some("300"));
    }

    #[test]
    fn height_only() {
        let map = parse_srcset("200h");
        assert_eq!(map.get("h").map(String::as_str), Some("200"));
    }

    #[test]
    fn zoom_only() {
        let map = parse_srcset("2x");
        assert_eq!(map.get("zoom").map(String::as_str), Some("2"));
    }

    #[test]
    fn fractional_zoom() {
        let map = parse_srcset("2.5x");
        assert_eq!(map.get("zoom").map(String::as_str), Some("2.5"));
    }

    #[test]
    fn full_example() {
        let map = parse_srcset("300w,200h,fit-crop,webp-90,qp-high,sharp-15");
        assert_eq!(map.get("w").map(String::as_str), Some("300"));
        assert_eq!(map.get("h").map(String::as_str), Some("200"));
        assert_eq!(map.get("mode").map(String::as_str), Some("crop"));
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("webp.quality").map(String::as_str), Some("90"));
        assert_eq!(map.get("qp").map(String::as_str), Some("high"));
        assert_eq!(map.get("f.sharpen").map(String::as_str), Some("15"));
    }

    #[test]
    fn fit_modes() {
        assert_eq!(
            parse_srcset("fit-pad").get("mode").map(String::as_str),
            Some("fit_pad")
        );
        assert_eq!(
            parse_srcset("fit-crop").get("mode").map(String::as_str),
            Some("crop")
        );
        assert_eq!(
            parse_srcset("fit-cover").get("mode").map(String::as_str),
            Some("crop")
        );
        assert_eq!(
            parse_srcset("fit-max").get("mode").map(String::as_str),
            Some("within")
        );
        assert_eq!(
            parse_srcset("fit-contain").get("mode").map(String::as_str),
            Some("within")
        );
        assert_eq!(
            parse_srcset("fit-distort").get("mode").map(String::as_str),
            Some("distort")
        );
        assert_eq!(
            parse_srcset("fit-fill").get("mode").map(String::as_str),
            Some("distort")
        );
    }

    #[test]
    fn format_only() {
        let map = parse_srcset("webp");
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert!(map.get("webp.quality").is_none());
    }

    #[test]
    fn jpeg_alias() {
        let map = parse_srcset("jpg-85");
        assert_eq!(map.get("format").map(String::as_str), Some("jpeg"));
        assert_eq!(map.get("jpeg.quality").map(String::as_str), Some("85"));
    }

    #[test]
    fn jpeg_quality() {
        let map = parse_srcset("jpeg-80");
        assert_eq!(map.get("format").map(String::as_str), Some("jpeg"));
        assert_eq!(map.get("jpeg.quality").map(String::as_str), Some("80"));
    }

    #[test]
    fn png_lossless() {
        let map = parse_srcset("png-lossless");
        assert_eq!(map.get("format").map(String::as_str), Some("png"));
        assert_eq!(map.get("png.lossless").map(String::as_str), Some("true"));
    }

    #[test]
    fn webp_lossless() {
        let map = parse_srcset("webp-lossless");
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("lossless").map(String::as_str), Some("true"));
    }

    #[test]
    fn jxl_distance() {
        let map = parse_srcset("jxl-d1.0");
        assert_eq!(map.get("format").map(String::as_str), Some("jxl"));
        assert_eq!(map.get("jxl.distance").map(String::as_str), Some("1"));
    }

    #[test]
    fn jxl_effort() {
        let map = parse_srcset("jxl-e7");
        assert_eq!(map.get("format").map(String::as_str), Some("jxl"));
        assert_eq!(map.get("jxl.effort").map(String::as_str), Some("7"));
    }

    #[test]
    fn jxl_lossless() {
        let map = parse_srcset("jxl-lossless");
        assert_eq!(map.get("format").map(String::as_str), Some("jxl"));
        assert_eq!(map.get("jxl.lossless").map(String::as_str), Some("true"));
    }

    #[test]
    fn avif_speed() {
        let map = parse_srcset("avif-s4");
        assert_eq!(map.get("format").map(String::as_str), Some("avif"));
        assert_eq!(map.get("avif.speed").map(String::as_str), Some("4"));
    }

    #[test]
    fn avif_quality() {
        let map = parse_srcset("avif-70");
        assert_eq!(map.get("format").map(String::as_str), Some("avif"));
        assert_eq!(map.get("avif.quality").map(String::as_str), Some("70"));
    }

    #[test]
    fn gif_format() {
        let map = parse_srcset("gif");
        assert_eq!(map.get("format").map(String::as_str), Some("gif"));
    }

    #[test]
    fn auto_format() {
        let map = parse_srcset("auto");
        assert_eq!(map.get("format").map(String::as_str), Some("auto"));
    }

    #[test]
    fn quality_profile() {
        let map = parse_srcset("qp-good");
        assert_eq!(map.get("qp").map(String::as_str), Some("good"));
    }

    #[test]
    fn quality_profile_lossless() {
        let map = parse_srcset("qp-lossless");
        assert_eq!(map.get("qp").map(String::as_str), Some("lossless"));
    }

    #[test]
    fn quality_profile_dpr() {
        let map = parse_srcset("qp-dpr-2x");
        assert_eq!(map.get("qp.dpr").map(String::as_str), Some("2"));
    }

    #[test]
    fn quality_profile_dpr_no_suffix() {
        let map = parse_srcset("qp-dpr-1.5");
        assert_eq!(map.get("qp.dpr").map(String::as_str), Some("1.5"));
    }

    #[test]
    fn standalone_lossless() {
        let map = parse_srcset("lossless");
        assert_eq!(map.get("lossless").map(String::as_str), Some("true"));
        assert_eq!(map.get("format").map(String::as_str), Some("auto"));
    }

    #[test]
    fn standalone_lossy() {
        let map = parse_srcset("lossy");
        assert_eq!(map.get("lossless").map(String::as_str), Some("false"));
        assert_eq!(map.get("format").map(String::as_str), Some("auto"));
    }

    #[test]
    fn upscale() {
        let map = parse_srcset("upscale");
        assert_eq!(map.get("scale").map(String::as_str), Some("both"));
    }

    #[test]
    fn sharpen() {
        let map = parse_srcset("sharpen-20");
        assert_eq!(map.get("f.sharpen").map(String::as_str), Some("20"));
    }

    #[test]
    fn sharp_alias() {
        let map = parse_srcset("sharp-15");
        assert_eq!(map.get("f.sharpen").map(String::as_str), Some("15"));
    }

    #[test]
    fn crop_percent() {
        let map = parse_srcset("crop-10-10-90-90");
        assert_eq!(map.get("c").map(String::as_str), Some("10,10,90,90"));
    }

    #[test]
    fn crop_fractional() {
        let map = parse_srcset("crop-10.5-20.5-80-90");
        assert_eq!(map.get("c").map(String::as_str), Some("10.5,20.5,80,90"));
    }

    #[test]
    fn crop_incomplete_ignored() {
        let map = parse_srcset("crop-10-20");
        assert!(map.get("c").is_none());
    }

    #[test]
    fn mixed_commands() {
        let map = parse_srcset("webp-lossless,2.5x,100w,100h,upscale");
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("lossless").map(String::as_str), Some("true"));
        assert_eq!(map.get("zoom").map(String::as_str), Some("2.5"));
        assert_eq!(map.get("w").map(String::as_str), Some("100"));
        assert_eq!(map.get("h").map(String::as_str), Some("100"));
        assert_eq!(map.get("scale").map(String::as_str), Some("both"));
    }

    #[test]
    fn case_insensitive() {
        let map = parse_srcset("WEBP-90,300W");
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("webp.quality").map(String::as_str), Some("90"));
        assert_eq!(map.get("w").map(String::as_str), Some("300"));
    }

    #[test]
    fn whitespace_tolerance() {
        let map = parse_srcset(" 300w , webp-90 , fit-crop ");
        assert_eq!(map.get("w").map(String::as_str), Some("300"));
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("mode").map(String::as_str), Some("crop"));
    }

    #[test]
    fn jpeg_progressive() {
        let map = parse_srcset("jpeg-85-progressive");
        assert_eq!(map.get("format").map(String::as_str), Some("jpeg"));
        assert_eq!(map.get("jpeg.quality").map(String::as_str), Some("85"));
        assert_eq!(
            map.get("jpeg.progressive").map(String::as_str),
            Some("progressive")
        );
    }

    #[test]
    fn jpeg_baseline() {
        let map = parse_srcset("jpeg-baseline");
        assert_eq!(map.get("format").map(String::as_str), Some("jpeg"));
        assert_eq!(
            map.get("jpeg.progressive").map(String::as_str),
            Some("baseline")
        );
    }

    #[test]
    fn png_quality_and_min_quality() {
        let map = parse_srcset("png-90-mq60");
        assert_eq!(map.get("format").map(String::as_str), Some("png"));
        assert_eq!(map.get("png.quality").map(String::as_str), Some("90"));
        assert_eq!(map.get("png.min_quality").map(String::as_str), Some("60"));
    }

    #[test]
    fn webp_quality_with_q_prefix() {
        let map = parse_srcset("webp-q85");
        assert_eq!(map.get("webp.quality").map(String::as_str), Some("85"));
    }

    #[test]
    fn jxl_quality_distance_effort() {
        let map = parse_srcset("jxl-d0.5-e9");
        assert_eq!(map.get("format").map(String::as_str), Some("jxl"));
        assert_eq!(map.get("jxl.distance").map(String::as_str), Some("0.5"));
        assert_eq!(map.get("jxl.effort").map(String::as_str), Some("9"));
    }

    // ── expand_srcset tests ──

    #[test]
    fn expand_no_srcset() {
        let result = expand_srcset("w=800&h=600");
        assert_eq!(result, "w=800&h=600");
    }

    #[test]
    fn expand_srcset_only() {
        let result = expand_srcset("srcset=300w,webp-90");
        assert!(result.contains("w=300"));
        assert!(result.contains("format=webp"));
        assert!(result.contains("webp.quality=90"));
    }

    #[test]
    fn expand_short_alias() {
        let result = expand_srcset("short=300w,webp");
        assert!(result.contains("w=300"));
        assert!(result.contains("format=webp"));
    }

    #[test]
    fn expand_preserves_other_keys() {
        let result = expand_srcset("other=yes&srcset=300w");
        assert!(result.contains("other=yes"));
        assert!(result.contains("w=300"));
    }

    #[test]
    fn expand_srcset_overrides_explicit() {
        let result = expand_srcset("w=100&srcset=300w");
        // srcset expansion should override the explicit w=100
        assert!(result.contains("w=300"));
        // The old w=100 should NOT be present
        assert!(!result.contains("w=100"));
    }

    #[test]
    fn expand_complex() {
        let result = expand_srcset("srcset=300w,200h,fit-crop,webp-90,qp-high,sharp-15&extra=1");
        assert!(result.contains("w=300"));
        assert!(result.contains("h=200"));
        assert!(result.contains("mode=crop"));
        assert!(result.contains("format=webp"));
        assert!(result.contains("webp.quality=90"));
        assert!(result.contains("qp=high"));
        assert!(result.contains("f.sharpen=15"));
        assert!(result.contains("extra=1"));
    }

    #[test]
    fn crop_with_other_commands() {
        let map = parse_srcset("gif,crop-20-30-90-100,2.5x,100w,100h");
        assert_eq!(map.get("format").map(String::as_str), Some("gif"));
        assert_eq!(map.get("c").map(String::as_str), Some("20,30,90,100"));
        assert_eq!(map.get("zoom").map(String::as_str), Some("2.5"));
        assert_eq!(map.get("w").map(String::as_str), Some("100"));
        assert_eq!(map.get("h").map(String::as_str), Some("100"));
    }

    #[test]
    fn format_keep_lossless() {
        let map = parse_srcset("webp-keep");
        assert_eq!(map.get("format").map(String::as_str), Some("webp"));
        assert_eq!(map.get("lossless").map(String::as_str), Some("keep"));
    }
}
