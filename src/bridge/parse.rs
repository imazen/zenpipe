//! Param extraction helpers and string-to-enum parsers for zennode bridge nodes.

use alloc::string::{String, ToString};

use zennode::NodeInstance;

use crate::error::PipeError;

pub(crate) fn param_u32(node: &dyn NodeInstance, name: &str) -> Result<u32, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_u32())
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid u32 param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

pub(crate) fn param_i32(node: &dyn NodeInstance, name: &str) -> Result<i32, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_i32())
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid i32 param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

pub(crate) fn param_str(node: &dyn NodeInstance, name: &str) -> Result<String, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid string param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

pub(crate) fn param_f32_opt(node: &dyn NodeInstance, name: &str) -> Option<f32> {
    node.get_param(name).and_then(|v| v.as_f32())
}

pub(crate) fn parse_constraint_mode(s: &str) -> Result<zenresize::ConstraintMode, PipeError> {
    match s {
        "distort" => Ok(zenresize::ConstraintMode::Distort),
        "fit" => Ok(zenresize::ConstraintMode::Fit),
        "within" => Ok(zenresize::ConstraintMode::Within),
        "fit_crop" | "crop" => Ok(zenresize::ConstraintMode::FitCrop),
        "within_crop" => Ok(zenresize::ConstraintMode::WithinCrop),
        "fit_pad" | "pad" => Ok(zenresize::ConstraintMode::FitPad),
        "within_pad" => Ok(zenresize::ConstraintMode::WithinPad),
        "pad_within" => Ok(zenresize::ConstraintMode::PadWithin),
        "aspect_crop" => Ok(zenresize::ConstraintMode::AspectCrop),
        _ => Err(PipeError::Op(alloc::format!(
            "bridge: unknown constraint mode '{s}'"
        ))),
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

pub(crate) fn parse_canvas_color(s: &str) -> Option<zenresize::CanvasColor> {
    let lower = s.to_ascii_lowercase();
    Some(match lower.as_str() {
        "transparent" | "" => zenresize::CanvasColor::Transparent,
        "white" => zenresize::CanvasColor::Srgb {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        "black" => zenresize::CanvasColor::Srgb {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        hex if hex.starts_with('#') => {
            let hex = &hex[1..];
            let bytes: alloc::vec::Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
                .collect();
            match bytes.len() {
                3 => zenresize::CanvasColor::Srgb {
                    r: bytes[0],
                    g: bytes[1],
                    b: bytes[2],
                    a: 255,
                },
                4 => zenresize::CanvasColor::Srgb {
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
        "robidoux" => Some(zenresize::Filter::Robidoux),
        "robidoux_sharp" => Some(zenresize::Filter::RobidouxSharp),
        "lanczos" | "lanczos3" => Some(zenresize::Filter::Lanczos),
        "lanczos2" => Some(zenresize::Filter::Lanczos2),
        "mitchell" => Some(zenresize::Filter::Mitchell),
        "catmull_rom" | "catrom" => Some(zenresize::Filter::CatmullRom),
        "hermite" => Some(zenresize::Filter::Hermite),
        "box" | "nearest" => Some(zenresize::Filter::Box),
        "triangle" | "linear" | "bilinear" => Some(zenresize::Filter::Triangle),
        "ginseng" => Some(zenresize::Filter::Ginseng),
        "cubic" => Some(zenresize::Filter::CubicBSpline),
        _ => None,
    }
}
