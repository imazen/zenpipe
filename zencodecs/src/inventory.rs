//! Structural inventories: a byte-exact map of an image file's parts.
//!
//! Dispatches to the detected format's decoder
//! ([`DecodeJob::inventory`](zencodec::decode::DecodeJob::inventory)). The
//! result lists every segment, chunk, box, item extent, block, trailer and gap
//! with its byte range and what the decoder does with it, so bytes no decoder
//! reads (private chunks, data after the logical end, slack in containers) can
//! be audited instead of passing through unseen. See [`zencodec::inventory`].

pub use zencodec::inventory::{
    Disposition, Inventory, InventoryError, MetadataKind, Part, PartId, PartKind, PartTag,
};

use crate::error::Result;
use crate::info::detect_format;
use crate::{AllowedFormats, CodecError, ImageFormat, Limits};
use whereat::at;

/// Inventory `data` with the decoder for its detected format.
///
/// `Ok(None)` when that format's decoder does not implement inventories yet.
/// Errors: unrecognised or disabled format, cancellation, resource limits.
/// Malformed files still produce an inventory; the parts the decoder could
/// not interpret are marked [`Disposition::Malformed`].
pub fn inventory(data: &[u8], registry: &AllowedFormats) -> Result<Option<Inventory>> {
    let format = detect_format(data).ok_or_else(|| at!(CodecError::UnrecognizedFormat))?;
    if !registry.can_decode(format) {
        return Err(at!(CodecError::DisabledFormat(format)));
    }
    inventory_format(data, format, None)
}

/// Inventory `data` as `format`, skipping detection, optionally under `limits`.
pub fn inventory_format(
    data: &[u8],
    format: ImageFormat,
    limits: Option<&Limits>,
) -> Result<Option<Inventory>> {
    crate::dyn_dispatch::dyn_inventory(format, data, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EncodeRequest;
    use imgref::ImgVec;
    use rgb::Rgba;

    fn gradient(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
        let px = (0..w * h)
            .map(|i| Rgba {
                r: (i % w * 9) as u8,
                g: (i / w * 13) as u8,
                b: 90,
                a: 255,
            })
            .collect();
        ImgVec::new(px, w, h)
    }

    fn declares_inventory(format: ImageFormat) -> bool {
        crate::dyn_dispatch::build_dyn_decoder_config(format, None, None)
            .map(|c| c.capabilities().inventory())
            .unwrap_or(false)
    }

    /// Every compiled-in format that zencodecs can encode: the dispatched
    /// inventory exists exactly when the decoder declares the capability, it
    /// validates, it attributes some bytes to image data, and junk appended to
    /// the file is never reported as consumed.
    #[test]
    fn dispatch_matches_declared_capability() {
        let img = gradient(24, 16);
        let allowed = AllowedFormats::all();
        let mut checked = 0;
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Avif,
            ImageFormat::Jxl,
        ] {
            if !allowed.can_encode(format) || !allowed.can_decode(format) {
                continue;
            }
            let bytes = EncodeRequest::new(format)
                .encode_full_frame_rgba8(img.as_ref())
                .unwrap_or_else(|e| panic!("{format}: encode: {e}"))
                .into_vec();
            let inv = inventory(&bytes, &allowed).unwrap_or_else(|e| panic!("{format}: {e}"));
            match (declares_inventory(format), inv) {
                (false, None) => {}
                (true, Some(inv)) => {
                    checked += 1;
                    inv.validate()
                        .unwrap_or_else(|e| panic!("{format}: {e}\n{inv}"));
                    assert_eq!(inv.input_len(), bytes.len() as u64, "{format}");
                    assert!(
                        inv.parts()
                            .iter()
                            .any(|p| p.disposition == Disposition::ImageData),
                        "{format}: no image data\n{inv}"
                    );
                    let mut junk = bytes.clone();
                    junk.extend_from_slice(b"appended-identifying-text");
                    let inv = inventory(&junk, &allowed)
                        .unwrap_or_else(|e| panic!("{format}+junk: {e}"))
                        .unwrap_or_else(|| panic!("{format}+junk: no inventory"));
                    inv.validate()
                        .unwrap_or_else(|e| panic!("{format}+junk: {e}\n{inv}"));
                    let tail = bytes.len() as u64;
                    let tail_consumed = inv.parts().iter().enumerate().any(|(i, p)| {
                        let leaf = !inv
                            .parts()
                            .iter()
                            .any(|c| c.parent.map(|x| x.index()) == Some(i));
                        leaf && p.range.end > tail && p.disposition.is_consumed()
                    });
                    assert!(
                        !tail_consumed,
                        "{format}: appended junk reported as consumed\n{inv}"
                    );
                }
                (declared, got) => panic!(
                    "{format}: decoder declares inventory = {declared} but dispatch returned {}",
                    if got.is_some() { "Some" } else { "None" }
                ),
            }
        }
        // 0 until codec inventory branches are patched in; grows as each lands.
        let _ = checked;
    }
}
