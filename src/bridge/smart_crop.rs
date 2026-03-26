//! Smart crop node converter — content-aware cropping via face detection + saliency.
//!
//! Converts `zenpipe.smart_crop` nodes to [`NodeOp::Analyze`] closures that:
//! 1. Materialize the upstream image
//! 2. Run face detection + saliency via zentract plugin
//! 3. Compute optimal crop via zenlayout::smart_crop
//! 4. Return a cropped source
//!
//! Requires the `nodes-faces` feature.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use zensally::bridge::build_smart_crop_input;
use zensally::{AnalysisOutput, FocusRegion, ImageRef, PixelFormat, SmartCropResult};
use zensally_zentract::ContentAnalyzer;
use zenlayout::smart_crop::{AspectRatio, CropConfig, CropMode, compute_crop};

use crate::Source;
use crate::analysis::AnalysisOutputs;
use crate::error::PipeError;
use crate::graph::NodeOp;
use crate::sources::{CropSource, MaterializedSource};

use super::parse::{param_f32_opt, param_str, param_u32};
use super::NodeConverter;

/// Node converter for `zenpipe.smart_crop` nodes.
pub struct SmartCropConverter;

impl NodeConverter for SmartCropConverter {
    fn can_convert(&self, schema_id: &str) -> bool {
        schema_id == "zenpipe.smart_crop"
    }

    fn convert(
        &self,
        node: &dyn zennode::NodeInstance,
    ) -> Result<NodeOp, PipeError> {
        let target_w = param_u32(node, "target_w").unwrap_or(0);
        let target_h = param_u32(node, "target_h").unwrap_or(0);
        let mode_str = param_str(node, "mode").unwrap_or_else(|_| String::from("minimal"));
        let enable_faces = param_str(node, "faces")
            .map(|s| s != "false")
            .unwrap_or(true);
        let enable_saliency = param_str(node, "saliency")
            .map(|s| s != "false")
            .unwrap_or(true);
        let _confidence = param_f32_opt(node, "confidence").unwrap_or(0.7);
        let focus_str = param_str(node, "focus").ok();

        // Parse manual focus regions from "x1-x2,y1-y2" format
        let manual_focus = parse_focus_regions(focus_str.as_deref());

        let crop_mode = match mode_str.as_str() {
            "maximal" => CropMode::Maximal,
            _ => CropMode::Minimal,
        };

        Ok(NodeOp::Analyze(Box::new(move |mat: MaterializedSource, outputs: &mut AnalysisOutputs| {
            let w = mat.width();
            let h = mat.height();
            let fmt = mat.format();

            // Convert pixel format
            let pixel_format = pixel_format_from_zenpipe(fmt);

            // Run detection if enabled
            let analysis = if enable_faces || enable_saliency {
                run_detection(mat.data(), w, h, pixel_format, enable_faces, enable_saliency)
            } else {
                AnalysisOutput {
                    faces: Vec::new(),
                    saliency: None,
                }
            };

            // Determine target aspect ratio
            let (tw, th) = if target_w > 0 && target_h > 0 {
                (target_w, target_h)
            } else {
                // Default: source aspect ratio (no crop)
                (w, h)
            };

            // Build smart crop input from detections + manual focus
            let summary = analysis.summary();
            let input = build_smart_crop_input(analysis, &manual_focus);

            let config = CropConfig {
                target_aspect: AspectRatio { w: tw, h: th },
                mode: crop_mode,
                ..CropConfig::default()
            };

            let crop_rect = compute_crop(
                w,
                h,
                &input.focus_regions,
                input.heatmap.as_ref(),
                &config,
            );

            // Store result in analysis outputs for serialization
            let result = SmartCropResult {
                detection: summary,
                crop: crop_rect.map(|r| zensally::CropRect {
                    x: r.x,
                    y: r.y,
                    w: r.width,
                    h: r.height,
                }),
                target_aspect: (tw, th),
                mode: mode_str.clone(),
                manual_focus: manual_focus.clone(),
            };
            outputs.insert("zensally.smart_crop", result);

            if let Some(rect) = crop_rect {
                if rect.x > 0 || rect.y > 0 || rect.width != w || rect.height != h {
                    return Ok(Box::new(CropSource::new(
                        Box::new(mat),
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                    )?));
                }
            }

            // No meaningful crop — pass through
            Ok(Box::new(mat))
        })))
    }

    fn convert_group(
        &self,
        nodes: &[&dyn zennode::NodeInstance],
    ) -> Result<NodeOp, PipeError> {
        if let Some(&node) = nodes.first() {
            self.convert(node)
        } else {
            Err(PipeError::Op(format!("empty smart_crop group")))
        }
    }
}

/// Parse focus regions from `"x1-x2,y1-y2"` format (issue #594).
fn parse_focus_regions(focus: Option<&str>) -> Vec<FocusRegion> {
    let Some(s) = focus else {
        return Vec::new();
    };
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Vec::new();
    }
    let x_parts: Vec<&str> = parts[0].split('-').collect();
    let y_parts: Vec<&str> = parts[1].split('-').collect();
    if x_parts.len() != 2 || y_parts.len() != 2 {
        return Vec::new();
    }

    let x1: f32 = x_parts[0].trim().parse().unwrap_or(0.0);
    let x2: f32 = x_parts[1].trim().parse().unwrap_or(100.0);
    let y1: f32 = y_parts[0].trim().parse().unwrap_or(0.0);
    let y2: f32 = y_parts[1].trim().parse().unwrap_or(100.0);

    vec![FocusRegion { x1, y1, x2, y2 }]
}

/// Convert zenpipe PixelFormat to zensally PixelFormat.
fn pixel_format_from_zenpipe(fmt: crate::format::PixelFormat) -> PixelFormat {
    let bpp = fmt.bytes_per_pixel();
    if bpp == 4 {
        PixelFormat::Rgba
    } else {
        PixelFormat::Rgb
    }
}

/// Run face detection + saliency through the zentract plugin.
fn run_detection(
    pixels: &[u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    faces: bool,
    saliency: bool,
) -> AnalysisOutput {
    let image = match ImageRef::new(pixels, width, height, format) {
        Ok(img) => img,
        Err(_) => {
            return AnalysisOutput {
                faces: Vec::new(),
                saliency: None,
            };
        }
    };

    let mut analyzer = match ContentAnalyzer::new() {
        Ok(a) => a,
        Err(_) => {
            return AnalysisOutput {
                faces: Vec::new(),
                saliency: None,
            };
        }
    };

    let result = analyzer.analyze(&image);

    AnalysisOutput {
        faces: if faces { result.faces } else { Vec::new() },
        saliency: if saliency { result.saliency } else { None },
    }
}
