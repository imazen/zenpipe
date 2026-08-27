//! Param extraction helpers and string-to-enum parsers for zennode bridge nodes.

use alloc::string::{String, ToString};

use zennode::NodeInstance;

#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;

pub(crate) fn param_u32(node: &dyn NodeInstance, name: &str) -> crate::PipeResult<u32> {
    node.get_param(name)
        .and_then(|v| v.as_u32())
        .ok_or_else(|| {
            at!(PipeError::Op(alloc::format!(
                "bridge: missing or invalid u32 param '{}' on '{}'",
                name,
                node.schema().id,
            )))
        })
}

pub(crate) fn param_u32_opt(node: &dyn NodeInstance, name: &str) -> Option<u32> {
    node.get_param(name).and_then(|v| v.as_u32())
}

pub(crate) fn param_i32(node: &dyn NodeInstance, name: &str) -> crate::PipeResult<i32> {
    node.get_param(name)
        .and_then(|v| v.as_i32())
        .ok_or_else(|| {
            at!(PipeError::Op(alloc::format!(
                "bridge: missing or invalid i32 param '{}' on '{}'",
                name,
                node.schema().id,
            )))
        })
}

pub(crate) fn param_str(node: &dyn NodeInstance, name: &str) -> crate::PipeResult<String> {
    node.get_param(name)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| {
            at!(PipeError::Op(alloc::format!(
                "bridge: missing or invalid string param '{}' on '{}'",
                name,
                node.schema().id,
            )))
        })
}

pub(crate) fn param_f32_opt(node: &dyn NodeInstance, name: &str) -> Option<f32> {
    node.get_param(name).and_then(|v| v.as_f32())
}

pub(crate) fn parse_constraint_mode(s: &str) -> crate::PipeResult<zenlayout::ConstraintMode> {
    match s {
        // ── zen-native names (scaling permission is encoded in the mode) ──
        "distort" => Ok(zenlayout::ConstraintMode::Distort),
        "fit" => Ok(zenlayout::ConstraintMode::Fit),
        "within" => Ok(zenlayout::ConstraintMode::Within),
        "fit_crop" => Ok(zenlayout::ConstraintMode::FitCrop),
        "within_crop" => Ok(zenlayout::ConstraintMode::WithinCrop),
        "fit_pad" => Ok(zenlayout::ConstraintMode::FitPad),
        "within_pad" => Ok(zenlayout::ConstraintMode::WithinPad),
        "pad_within" => Ok(zenlayout::ConstraintMode::PadWithin),
        "aspect_crop" | "aspectcrop" => Ok(zenlayout::ConstraintMode::AspectCrop),
        // ── RIAPI fit-mode names, resolved with the RIAPI default
        // `scale=down` (never upscale). The geometry bridge intercepts these
        // BEFORE this function when a `scale=` value is present and composes
        // the full mode×scale matrix with dimension gating; this fallback
        // covers callers without dimensions (and encodes the correct
        // downscale-only default, unlike the pre-2026-07 aliases which
        // mapped `crop`→fit_crop / `pad`→fit_pad and upscaled where
        // ImageResizer/imageflow would not).
        "max" | "none" => Ok(zenlayout::ConstraintMode::Within),
        "crop" => Ok(zenlayout::ConstraintMode::WithinCrop),
        "pad" => Ok(zenlayout::ConstraintMode::WithinPad),
        "stretch" => Ok(zenlayout::ConstraintMode::Distort),
        "larger_than" => Ok(zenlayout::ConstraintMode::LargerThan),
        _ => Err(at!(PipeError::Op(alloc::format!(
            "bridge: unknown constraint mode '{s}'"
        )))),
    }
}

pub(crate) fn parse_gravity_anchor(s: &str) -> Option<(f32, f32)> {
    let lower = s.to_ascii_lowercase();
    // RIAPI also accepts `anchor=x,y` as percentages (0–100).
    if let Some((xs, ys)) = lower.split_once(',') {
        let x = xs.trim().parse::<f32>().ok()?;
        let y = ys.trim().parse::<f32>().ok()?;
        if x.is_finite() && y.is_finite() {
            return Some(((x / 100.0).clamp(0.0, 1.0), (y / 100.0).clamp(0.0, 1.0)));
        }
        return None;
    }
    Some(match lower.as_str() {
        // zen spellings + ImageResizer 4 spellings.
        "center" | "middlecenter" => (0.5, 0.5),
        "top_left" | "topleft" => (0.0, 0.0),
        "top" | "topcenter" => (0.5, 0.0),
        "top_right" | "topright" => (1.0, 0.0),
        "left" | "middleleft" => (0.0, 0.5),
        "right" | "middleright" => (1.0, 0.5),
        "bottom_left" | "bottomleft" => (0.0, 1.0),
        "bottom" | "bottomcenter" => (0.5, 1.0),
        "bottom_right" | "bottomright" => (1.0, 1.0),
        _ => return None,
    })
}

pub(crate) fn parse_canvas_color(s: &str) -> Option<zenlayout::CanvasColor> {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "transparent" | "" => Some(zenlayout::CanvasColor::Transparent),
        hex if hex.starts_with('#') => parse_hex_color(&hex[1..]),
        name => {
            // Named CSS colors first (RIAPI accepts the CSS3 table),
            // then bare hex without '#' (RIAPI: `bgcolor=FFAAEE`).
            if let Some([r, g, b]) = css_named_color(name) {
                Some(zenlayout::CanvasColor::Srgb { r, g, b, a: 255 })
            } else {
                parse_hex_color(name)
            }
        }
    }
}

/// Parse a matte color string to opaque sRGB bytes (`[r, g, b]`).
/// Transparent/unknown values yield `None` (no matte).
#[cfg(feature = "job")]
pub(crate) fn parse_matte_rgb(s: &str) -> Option<[u8; 3]> {
    match parse_canvas_color(s)? {
        zenlayout::CanvasColor::Srgb { r, g, b, .. } => Some([r, g, b]),
        _ => None,
    }
}

/// Parse 3/4/6/8-digit hex (no `#`). Shorthand nibbles double (`f80` →
/// `ff8800`); missing alpha = opaque. Matches imageflow_helpers colors.rs.
fn parse_hex_color(hex: &str) -> Option<zenlayout::CanvasColor> {
    if !hex.is_ascii() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let (r, g, b, a) = match hex.len() {
        3 => (nibble(0)?, nibble(1)?, nibble(2)?, 15),
        4 => (nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?),
        6 => {
            return Some(zenlayout::CanvasColor::Srgb {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: 255,
            });
        }
        8 => {
            return Some(zenlayout::CanvasColor::Srgb {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            });
        }
        _ => return None,
    };
    // Shorthand: double each nibble.
    Some(zenlayout::CanvasColor::Srgb {
        r: r * 17,
        g: g * 17,
        b: b * 17,
        a: a * 17,
    })
}

/// CSS3 extended named colors (the table RIAPI/ImageResizer accepts for
/// `bgcolor=`). Sorted for binary search; values per CSS Color Module 3.
#[rustfmt::skip]
static CSS_COLORS: &[(&str, [u8; 3])] = &[
    ("aliceblue", [240, 248, 255]), ("antiquewhite", [250, 235, 215]),
    ("aqua", [0, 255, 255]), ("aquamarine", [127, 255, 212]),
    ("azure", [240, 255, 255]), ("beige", [245, 245, 220]),
    ("bisque", [255, 228, 196]), ("black", [0, 0, 0]),
    ("blanchedalmond", [255, 235, 205]), ("blue", [0, 0, 255]),
    ("blueviolet", [138, 43, 226]), ("brown", [165, 42, 42]),
    ("burlywood", [222, 184, 135]), ("cadetblue", [95, 158, 160]),
    ("chartreuse", [127, 255, 0]), ("chocolate", [210, 105, 30]),
    ("coral", [255, 127, 80]), ("cornflowerblue", [100, 149, 237]),
    ("cornsilk", [255, 248, 220]), ("crimson", [220, 20, 60]),
    ("cyan", [0, 255, 255]), ("darkblue", [0, 0, 139]),
    ("darkcyan", [0, 139, 139]), ("darkgoldenrod", [184, 134, 11]),
    ("darkgray", [169, 169, 169]), ("darkgreen", [0, 100, 0]),
    ("darkgrey", [169, 169, 169]), ("darkkhaki", [189, 183, 107]),
    ("darkmagenta", [139, 0, 139]), ("darkolivegreen", [85, 107, 47]),
    ("darkorange", [255, 140, 0]), ("darkorchid", [153, 50, 204]),
    ("darkred", [139, 0, 0]), ("darksalmon", [233, 150, 122]),
    ("darkseagreen", [143, 188, 143]), ("darkslateblue", [72, 61, 139]),
    ("darkslategray", [47, 79, 79]), ("darkslategrey", [47, 79, 79]),
    ("darkturquoise", [0, 206, 209]), ("darkviolet", [148, 0, 211]),
    ("deeppink", [255, 20, 147]), ("deepskyblue", [0, 191, 255]),
    ("dimgray", [105, 105, 105]), ("dimgrey", [105, 105, 105]),
    ("dodgerblue", [30, 144, 255]), ("firebrick", [178, 34, 34]),
    ("floralwhite", [255, 250, 240]), ("forestgreen", [34, 139, 34]),
    ("fuchsia", [255, 0, 255]), ("gainsboro", [220, 220, 220]),
    ("ghostwhite", [248, 248, 255]), ("gold", [255, 215, 0]),
    ("goldenrod", [218, 165, 32]), ("gray", [128, 128, 128]),
    ("green", [0, 128, 0]), ("greenyellow", [173, 255, 47]),
    ("grey", [128, 128, 128]), ("honeydew", [240, 255, 240]),
    ("hotpink", [255, 105, 180]), ("indianred", [205, 92, 92]),
    ("indigo", [75, 0, 130]), ("ivory", [255, 255, 240]),
    ("khaki", [240, 230, 140]), ("lavender", [230, 230, 250]),
    ("lavenderblush", [255, 240, 245]), ("lawngreen", [124, 252, 0]),
    ("lemonchiffon", [255, 250, 205]), ("lightblue", [173, 216, 230]),
    ("lightcoral", [240, 128, 128]), ("lightcyan", [224, 255, 255]),
    ("lightgoldenrodyellow", [250, 250, 210]), ("lightgray", [211, 211, 211]),
    ("lightgreen", [144, 238, 144]), ("lightgrey", [211, 211, 211]),
    ("lightpink", [255, 182, 193]), ("lightsalmon", [255, 160, 122]),
    ("lightseagreen", [32, 178, 170]), ("lightskyblue", [135, 206, 250]),
    ("lightslategray", [119, 136, 153]), ("lightslategrey", [119, 136, 153]),
    ("lightsteelblue", [176, 196, 222]), ("lightyellow", [255, 255, 224]),
    ("lime", [0, 255, 0]), ("limegreen", [50, 205, 50]),
    ("linen", [250, 240, 230]), ("magenta", [255, 0, 255]),
    ("maroon", [128, 0, 0]), ("mediumaquamarine", [102, 205, 170]),
    ("mediumblue", [0, 0, 205]), ("mediumorchid", [186, 85, 211]),
    ("mediumpurple", [147, 112, 219]), ("mediumseagreen", [60, 179, 113]),
    ("mediumslateblue", [123, 104, 238]), ("mediumspringgreen", [0, 250, 154]),
    ("mediumturquoise", [72, 209, 204]), ("mediumvioletred", [199, 21, 133]),
    ("midnightblue", [25, 25, 112]), ("mintcream", [245, 255, 250]),
    ("mistyrose", [255, 228, 225]), ("moccasin", [255, 228, 181]),
    ("navajowhite", [255, 222, 173]), ("navy", [0, 0, 128]),
    ("oldlace", [253, 245, 230]), ("olive", [128, 128, 0]),
    ("olivedrab", [107, 142, 35]), ("orange", [255, 165, 0]),
    ("orangered", [255, 69, 0]), ("orchid", [218, 112, 214]),
    ("palegoldenrod", [238, 232, 170]), ("palegreen", [152, 251, 152]),
    ("paleturquoise", [175, 238, 238]), ("palevioletred", [219, 112, 147]),
    ("papayawhip", [255, 239, 213]), ("peachpuff", [255, 218, 185]),
    ("peru", [205, 133, 63]), ("pink", [255, 192, 203]),
    ("plum", [221, 160, 221]), ("powderblue", [176, 224, 230]),
    ("purple", [128, 0, 128]), ("rebeccapurple", [102, 51, 153]),
    ("red", [255, 0, 0]), ("rosybrown", [188, 143, 143]),
    ("royalblue", [65, 105, 225]), ("saddlebrown", [139, 69, 19]),
    ("salmon", [250, 128, 114]), ("sandybrown", [244, 164, 96]),
    ("seagreen", [46, 139, 87]), ("seashell", [255, 245, 238]),
    ("sienna", [160, 82, 45]), ("silver", [192, 192, 192]),
    ("skyblue", [135, 206, 235]), ("slateblue", [106, 90, 205]),
    ("slategray", [112, 128, 144]), ("slategrey", [112, 128, 144]),
    ("snow", [255, 250, 250]), ("springgreen", [0, 255, 127]),
    ("steelblue", [70, 130, 180]), ("tan", [210, 180, 140]),
    ("teal", [0, 128, 128]), ("thistle", [216, 191, 216]),
    ("tomato", [255, 99, 71]), ("turquoise", [64, 224, 208]),
    ("violet", [238, 130, 238]), ("wheat", [245, 222, 179]),
    ("white", [255, 255, 255]), ("whitesmoke", [245, 245, 245]),
    ("yellow", [255, 255, 0]), ("yellowgreen", [154, 205, 50]),
];

pub(crate) fn css_named_color(name: &str) -> Option<[u8; 3]> {
    CSS_COLORS
        .binary_search_by_key(&name, |(n, _)| n)
        .ok()
        .map(|i| CSS_COLORS[i].1)
}

pub(crate) fn parse_filter_opt(s: &str) -> Option<zenresize::Filter> {
    if s.is_empty() {
        return None;
    }
    match s {
        // Robidoux family
        "robidoux" => Some(zenresize::Filter::Robidoux),
        "robidoux_sharp" => Some(zenresize::Filter::RobidouxSharp),
        "robidoux_fast" => Some(zenresize::Filter::RobidouxFast),
        // Lanczos family
        "lanczos" | "lanczos3" => Some(zenresize::Filter::Lanczos),
        "lanczos_sharp" => Some(zenresize::Filter::LanczosSharp),
        "lanczos2" => Some(zenresize::Filter::Lanczos2),
        "lanczos2_sharp" => Some(zenresize::Filter::Lanczos2Sharp),
        "raw_lanczos3" => Some(zenresize::Filter::RawLanczos3),
        "raw_lanczos3_sharp" => Some(zenresize::Filter::RawLanczos3Sharp),
        "raw_lanczos2" => Some(zenresize::Filter::RawLanczos2),
        "raw_lanczos2_sharp" => Some(zenresize::Filter::RawLanczos2Sharp),
        // Cubic family
        "cubic" => Some(zenresize::Filter::Cubic),
        "cubic_sharp" => Some(zenresize::Filter::CubicSharp),
        "cubic_fast" => Some(zenresize::Filter::CubicFast),
        "cubic_b_spline" | "cubic_bspline" => Some(zenresize::Filter::CubicBSpline),
        "mitchell" => Some(zenresize::Filter::Mitchell),
        "mitchell_fast" => Some(zenresize::Filter::MitchellFast),
        "catmull_rom" | "catrom" => Some(zenresize::Filter::CatmullRom),
        "catmull_rom_fast" => Some(zenresize::Filter::CatmullRomFast),
        "catmull_rom_fast_sharp" => Some(zenresize::Filter::CatmullRomFastSharp),
        "hermite" => Some(zenresize::Filter::Hermite),
        "n_cubic" | "ncubic" => Some(zenresize::Filter::NCubic),
        "n_cubic_sharp" | "ncubic_sharp" => Some(zenresize::Filter::NCubicSharp),
        // Ginseng / Jinc
        "ginseng" => Some(zenresize::Filter::Ginseng),
        "ginseng_sharp" => Some(zenresize::Filter::GinsengSharp),
        "jinc" => Some(zenresize::Filter::Jinc),
        // Simple filters
        "box" | "nearest" => Some(zenresize::Filter::Box),
        "triangle" | "linear" | "bilinear" => Some(zenresize::Filter::Triangle),
        "fastest" => Some(zenresize::Filter::Fastest),
        // Legacy
        "legacy_idct" => Some(zenresize::Filter::LegacyIDCTFilter),
        _ => None,
    }
}
