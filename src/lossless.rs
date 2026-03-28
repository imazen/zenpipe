//! Lossless JPEG fast path for orient-only pipelines.
//!
//! When a pipeline is JPEG → orient → JPEG with no pixel-level operations,
//! this module skips pixel decoding entirely and uses
//! [`zenjpeg::lossless::transform()`] on DCT coefficients. This is ~10x faster
//! than the full decode → transform → encode path and produces zero generation
//! loss.
//!
//! # Usage
//!
//! Call [`try_lossless_jpeg()`] before decoding the source image. If it returns
//! `Some(data)`, the lossless path succeeded and the result is ready for output.
//! If it returns `None`, fall through to the normal streaming pipeline.
//!
//! # Supported operations
//!
//! Only pure orientation transforms are handled losslessly:
//! - `zenlayout.orient` (auto-orient from EXIF tag)
//! - `zenlayout.flip_h`, `zenlayout.flip_v`
//! - `zenlayout.rotate_90`, `zenlayout.rotate_180`, `zenlayout.rotate_270`
//!
//! Any other processing node (resize, crop, filter, composite, etc.) causes
//! a fallback to the normal pixel pipeline.

use alloc::boxed::Box;
use alloc::vec::Vec;

use enough::Stop;
use zenjpeg::lossless::{EdgeHandling, LosslessTransform, TransformConfig};
use zennode::NodeInstance;

use crate::bridge::EncodeConfig;
use crate::error::PipeError;

/// Result of a successful lossless JPEG transform.
#[derive(Debug)]
pub struct LosslessResult {
    /// The transformed JPEG bytes, ready for output.
    pub data: Vec<u8>,
    /// Output image width (after transform).
    pub width: u32,
    /// Output image height (after transform).
    pub height: u32,
}

/// Try to execute a pipeline via lossless JPEG transforms.
///
/// Returns `Ok(Some(result))` if all processing nodes are pure orientation
/// transforms and the source/output are both JPEG. Returns `Ok(None)` if
/// the pipeline requires pixel decoding (caller should fall through to
/// the normal streaming path).
///
/// # Arguments
///
/// * `source_data` — Raw source image bytes (must be JPEG).
/// * `nodes` — The full node list (decode, pixel, and encode phases).
///   Only pixel-phase nodes are inspected; decode/encode nodes are skipped.
/// * `encode_config` — Encode configuration (used to check output format).
/// * `exif_orientation` — EXIF orientation tag (1-8) from source probing.
/// * `stop` — Cooperative cancellation token.
///
/// # Errors
///
/// Returns `Err` only if the lossless transform itself fails (corrupt JPEG,
/// etc.). A pipeline that can't be done losslessly returns `Ok(None)`.
pub fn try_lossless_jpeg(
    source_data: &[u8],
    nodes: &[Box<dyn NodeInstance>],
    encode_config: &EncodeConfig,
    exif_orientation: u8,
    stop: &dyn Stop,
) -> Result<Option<LosslessResult>, PipeError> {
    // 1. Source must be JPEG (check magic bytes).
    if !is_jpeg(source_data) {
        return Ok(None);
    }

    // 2. Output must be JPEG (explicit JPEG, "keep", or auto/None).
    if !target_is_jpeg(encode_config) {
        return Ok(None);
    }

    // 3. Separate nodes by role and classify pixel-processing nodes.
    let transform = match classify_nodes(nodes, exif_orientation) {
        Some(t) => t,
        None => return Ok(None),
    };

    // 4. Identity transform — return source unchanged.
    if transform == LosslessTransform::None {
        // Probe dimensions from JPEG headers for the result.
        let (w, h) = probe_jpeg_dimensions(source_data).unwrap_or((0, 0));
        return Ok(Some(LosslessResult {
            data: source_data.to_vec(),
            width: w,
            height: h,
        }));
    }

    // 5. Execute the lossless transform on DCT coefficients.
    let config = TransformConfig {
        transform,
        edge_handling: EdgeHandling::TrimPartialBlocks,
    };

    let result = zenjpeg::lossless::transform(source_data, &config, stop)
        .map_err(|e| PipeError::Op(alloc::format!("lossless JPEG transform failed: {e}")))?;

    // 6. Probe output dimensions.
    let (w, h) = probe_jpeg_dimensions(&result).unwrap_or_else(|| {
        // Fallback: compute from source dims + transform.
        let (sw, sh) = probe_jpeg_dimensions(source_data).unwrap_or((0, 0));
        transform.output_dimensions(sw, sh)
    });

    Ok(Some(LosslessResult {
        data: result,
        width: w,
        height: h,
    }))
}

/// Classify all pixel-processing nodes as a single composed lossless transform.
///
/// Returns `None` if any node requires pixel decoding (resize, crop, filter, etc.).
/// Returns `Some(LosslessTransform::None)` for an empty pipeline.
///
/// Decode and encode phase nodes are skipped (they configure the codec, not pixels).
fn classify_nodes(
    nodes: &[Box<dyn NodeInstance>],
    exif_orientation: u8,
) -> Option<LosslessTransform> {
    use zennode::NodeRole;

    let mut combined = LosslessTransform::None;

    for node in nodes {
        let role = node.schema().role;
        // Skip decode/encode phase nodes — they don't affect pixels.
        if matches!(role, NodeRole::Encode | NodeRole::Decode) {
            continue;
        }

        let schema_id = node.schema().id;
        let step_transform = match schema_id {
            "zenlayout.flip_h" => LosslessTransform::FlipHorizontal,
            "zenlayout.flip_v" => LosslessTransform::FlipVertical,
            "zenlayout.rotate_90" => LosslessTransform::Rotate90,
            "zenlayout.rotate_180" => LosslessTransform::Rotate180,
            "zenlayout.rotate_270" => LosslessTransform::Rotate270,
            "zenlayout.orient" => {
                // Orient node: read the orientation field, or use source EXIF.
                // orientation=0 is a sentinel for "use EXIF orientation at decode time".
                let exif_val = node
                    .get_param("orientation")
                    .and_then(|v| v.as_u32())
                    .map(|v| v as u8)
                    .filter(|&v| v != 0)
                    .unwrap_or(exif_orientation);
                orientation_to_lossless(exif_val)?
            }
            // Any other pixel-processing node → not lossless.
            _ => return None,
        };

        combined = combined.then(step_transform);
    }

    Some(combined)
}

/// Map an EXIF orientation tag (1-8) to a `LosslessTransform`.
fn orientation_to_lossless(exif_orientation: u8) -> Option<LosslessTransform> {
    LosslessTransform::from_exif_orientation(exif_orientation)
}

/// Check if the target output format is JPEG (or will default to JPEG).
///
/// Returns true for explicit "jpeg", "keep" (same as source), or None/auto
/// (which defaults to source format for JPEG inputs).
fn target_is_jpeg(encode_config: &EncodeConfig) -> bool {
    matches!(
        encode_config.format.as_deref(),
        Some("jpeg") | Some("jpg") | Some("keep") | None
    )
}

/// Check JPEG magic bytes (SOI marker: 0xFF 0xD8).
fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

/// Probe JPEG dimensions from SOF marker without full decode.
///
/// Scans for SOF0/SOF1/SOF2 markers and reads the height/width fields.
/// Returns `None` if no SOF marker is found.
fn probe_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // Skip SOI (0xFF 0xD8)
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // SOF markers: SOF0 (0xC0), SOF1 (0xC1), SOF2 (0xC2)
        if matches!(marker, 0xC0..=0xC2) {
            // SOF structure: marker (2) + length (2) + precision (1) + height (2) + width (2)
            if i + 9 <= data.len() {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            return None;
        }
        // SOS (0xDA) — start of scan, stop searching
        if marker == 0xDA {
            break;
        }
        // Skip over segment: read length and advance
        if i + 3 < data.len() {
            let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + seg_len;
        } else {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn is_jpeg_valid() {
        assert!(is_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!is_jpeg(&[0x89, 0x50, 0x4E, 0x47])); // PNG
        assert!(!is_jpeg(&[0xFF])); // too short
        assert!(!is_jpeg(&[]));
    }

    #[test]
    fn target_is_jpeg_checks() {
        let mut config = EncodeConfig::default();
        assert!(target_is_jpeg(&config)); // None → true

        config.format = Some("jpeg".to_string());
        assert!(target_is_jpeg(&config));

        config.format = Some("jpg".to_string());
        assert!(target_is_jpeg(&config));

        config.format = Some("keep".to_string());
        assert!(target_is_jpeg(&config));

        config.format = Some("png".to_string());
        assert!(!target_is_jpeg(&config));

        config.format = Some("webp".to_string());
        assert!(!target_is_jpeg(&config));
    }

    #[test]
    fn orientation_to_lossless_mapping() {
        assert_eq!(orientation_to_lossless(1), Some(LosslessTransform::None));
        assert_eq!(
            orientation_to_lossless(6),
            Some(LosslessTransform::Rotate90)
        );
        assert_eq!(
            orientation_to_lossless(8),
            Some(LosslessTransform::Rotate270)
        );
        assert_eq!(orientation_to_lossless(0), None);
        assert_eq!(orientation_to_lossless(9), None);
    }

    #[test]
    fn classify_empty_nodes_is_identity() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![];
        assert_eq!(classify_nodes(&nodes, 1), Some(LosslessTransform::None));
    }

    #[test]
    fn compose_via_then() {
        // Rotate90 then FlipH = Transpose
        let a = LosslessTransform::Rotate90;
        let b = LosslessTransform::FlipHorizontal;
        let combined = a.then(b);
        assert_eq!(combined, LosslessTransform::Transpose);
    }

    #[test]
    fn probe_jpeg_dimensions_minimal() {
        // Construct a minimal JPEG with SOF0:
        // SOI + SOF0(marker=0xFFC0, len=8+1, precision=8, h=480, w=640)
        let mut data = vec![0xFF, 0xD8]; // SOI
        data.extend_from_slice(&[0xFF, 0xC0]); // SOF0 marker
        data.extend_from_slice(&[0x00, 0x0B]); // length = 11
        data.push(8); // precision
        data.extend_from_slice(&[0x01, 0xE0]); // height = 480
        data.extend_from_slice(&[0x02, 0x80]); // width = 640
        // (remaining SOF fields omitted — probe only reads w/h)

        assert_eq!(probe_jpeg_dimensions(&data), Some((640, 480)));
    }

    #[test]
    fn probe_jpeg_dimensions_not_jpeg() {
        assert_eq!(probe_jpeg_dimensions(&[0x89, 0x50, 0x4E, 0x47]), None);
    }
}
