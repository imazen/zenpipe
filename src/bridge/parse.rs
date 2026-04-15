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
        "distort" => Ok(zenlayout::ConstraintMode::Distort),
        "fit" => Ok(zenlayout::ConstraintMode::Fit),
        "within" => Ok(zenlayout::ConstraintMode::Within),
        "fit_crop" | "crop" => Ok(zenlayout::ConstraintMode::FitCrop),
        "within_crop" => Ok(zenlayout::ConstraintMode::WithinCrop),
        "fit_pad" | "pad" => Ok(zenlayout::ConstraintMode::FitPad),
        "within_pad" => Ok(zenlayout::ConstraintMode::WithinPad),
        "pad_within" => Ok(zenlayout::ConstraintMode::PadWithin),
        "aspect_crop" => Ok(zenlayout::ConstraintMode::AspectCrop),
        // "larger_than" not yet in zenresize
        "larger_than" => Err(at!(PipeError::Op(
            "larger_than mode not yet supported".into(),
        ))),
        _ => Err(at!(PipeError::Op(alloc::format!(
            "bridge: unknown constraint mode '{s}'"
        )))),
    }
}

pub(crate) fn parse_gravity_anchor(s: &str) -> Option<(f32, f32)> {
    Some(match s {
        "center" => (0.5, 0.5),
        "top_left" => (0.0, 0.0),
        "top" => (0.5, 0.0),
        "top_right" => (1.0, 0.0),
        "left" => (0.0, 0.5),
        "right" => (1.0, 0.5),
        "bottom_left" => (0.0, 1.0),
        "bottom" => (0.5, 1.0),
        "bottom_right" => (1.0, 1.0),
        _ => return None,
    })
}

pub(crate) fn parse_canvas_color(s: &str) -> Option<zenlayout::CanvasColor> {
    let lower = s.to_ascii_lowercase();
    Some(match lower.as_str() {
        "transparent" | "" => zenlayout::CanvasColor::Transparent,
        "white" => zenlayout::CanvasColor::Srgb {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        "black" => zenlayout::CanvasColor::Srgb {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        hex if hex.starts_with('#') => {
            let hex = &hex[1..];
            let bytes: alloc::vec::Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|i| {
                    hex.get(i..i + 2)
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                })
                .collect();
            match bytes.len() {
                3 => zenlayout::CanvasColor::Srgb {
                    r: bytes[0],
                    g: bytes[1],
                    b: bytes[2],
                    a: 255,
                },
                4 => zenlayout::CanvasColor::Srgb {
                    r: bytes[0],
                    g: bytes[1],
                    b: bytes[2],
                    a: bytes[3],
                },
                _ => return None,
            }
        }
        _ => return None,
    })
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
