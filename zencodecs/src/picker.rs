//! Optional MLP-backed [`FormatPicker`] over zenpicker (the `picker` feature).
//!
//! `zenpicker` is codec-agnostic — it returns its own `CodecFamily` from an MLP
//! over zenanalyze image features. This adapter maps a zencodecs candidate set to
//! the picker's allowed-family mask, runs the meta-picker, and maps the chosen
//! family back to an [`ImageFormat`], so it plugs straight into
//! [`select_format_from_intent_with_picker`](crate::select_format_from_intent_with_picker).
//!
//! The feature vector is the caller's to produce (e.g. via zenanalyze) and is
//! passed through [`FormatPicker::pick`] — this crate takes no analysis
//! dependency, and the picker only re-ranks the candidates zencodecs already
//! proved valid (it can never widen the allowed set).

use crate::select::FormatPicker;
use crate::{CodecError, Result};
use alloc::boxed::Box;
use zencodec::ImageFormat;
use zenpicker::{AllowedFamilies, CodecFamily, MetaPicker};
use zenpredict::Model;

/// The picker's [`CodecFamily`] for a zencodecs [`ImageFormat`], if the format is
/// one the meta-picker ranks. Formats outside {jpeg, webp, jxl, avif, png, gif}
/// (bmp, pnm, tiff, …) have no family and are simply not offered to the picker.
fn family_of(format: ImageFormat) -> Option<CodecFamily> {
    Some(match format {
        ImageFormat::Jpeg => CodecFamily::Jpeg,
        ImageFormat::WebP => CodecFamily::Webp,
        ImageFormat::Jxl => CodecFamily::Jxl,
        ImageFormat::Avif => CodecFamily::Avif,
        ImageFormat::Png => CodecFamily::Png,
        ImageFormat::Gif => CodecFamily::Gif,
        _ => return None,
    })
}

/// The canonical [`ImageFormat`] for a picker [`CodecFamily`] — the inverse of
/// [`family_of`] over the six picker families. `None` for a future family this
/// build doesn't map (`CodecFamily` is `#[non_exhaustive]`).
fn format_of(family: CodecFamily) -> Option<ImageFormat> {
    Some(match family {
        CodecFamily::Jpeg => ImageFormat::Jpeg,
        CodecFamily::Webp => ImageFormat::WebP,
        CodecFamily::Jxl => ImageFormat::Jxl,
        CodecFamily::Avif => ImageFormat::Avif,
        CodecFamily::Png => ImageFormat::Png,
        CodecFamily::Gif => ImageFormat::Gif,
        _ => return None,
    })
}

/// A [`FormatPicker`] backed by a zenpicker meta-model (an MLP over zenanalyze
/// features).
///
/// Construct once from baked model bytes, then pass `&self` to
/// [`select_format_from_intent_with_picker`](crate::select_format_from_intent_with_picker)
/// with a feature vector the model expects.
pub struct MlpFormatPicker {
    model: Box<Model>,
}

impl MlpFormatPicker {
    /// Load a baked codec-family meta-picker model (ZNPR). Fails only if the
    /// bytes aren't a parseable model; the family order is trusted (call
    /// [`validate_family_order`](Self::validate_family_order) to verify it).
    pub fn from_model_bytes(bytes: &[u8]) -> Result<Self> {
        let model = Model::from_bytes(bytes).map_err(|e| {
            whereat::at!(CodecError::InvalidInput(alloc::format!(
                "picker model parse: {e:?}"
            )))
        })?;
        Ok(Self {
            model: Box::new(model),
        })
    }

    /// Best-effort check that the model's declared family order matches the
    /// `jpeg,webp,jxl,avif,png,gif` layout this adapter assumes (output index
    /// `i` ↔ that family). Optional: [`from_model_bytes`](Self::from_model_bytes)
    /// trusts the order so it still accepts models baked before the current
    /// `family_order` metadata convention; call this to verify once models are
    /// re-baked with it.
    pub fn validate_family_order(&self) -> Result<()> {
        MetaPicker::new(&self.model)
            .validate_family_order()
            .map_err(|e| {
                whereat::at!(CodecError::InvalidInput(alloc::format!(
                    "family order: {e:?}"
                )))
            })
    }
}

impl FormatPicker for MlpFormatPicker {
    fn pick(&self, features: &[f32], candidates: &[ImageFormat]) -> Option<ImageFormat> {
        // Candidates → allowed-family mask. Formats with no picker family are
        // dropped; the picker can only choose among the offered families.
        let mut allowed = AllowedFamilies::none();
        for &fmt in candidates {
            if let Some(fam) = family_of(fmt) {
                allowed = allowed.allow(fam);
            }
        }
        // A fresh picker over the owned model — one selection per image, so the
        // scratch allocation is negligible against the encode it informs.
        let mut picker = MetaPicker::new(&self.model);
        let family = picker.pick(features, &allowed).ok()??;
        let chosen = format_of(family)?;
        // Defensive: only ever return a format that is actually a candidate.
        candidates.iter().copied().find(|&c| c == chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vendored 7.7 KB meta-picker (jpeg/webp/jxl/avif/png), from the zenanalyze
    // bake `zenpicker_meta_v0.5_5codec_2026-05-06`. A fixed test fixture.
    const MODEL: &[u8] = include_bytes!("../tests/data/zenpicker_meta_v0_5_5codec.bin");

    #[test]
    fn loads_model_and_picks_a_candidate() {
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        // The model takes 20 features; a valid-shaped synthetic vector.
        let features = [0.3f32; 20];
        let cands = [ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Png];
        let pick = picker.pick(&features, &cands);
        assert!(
            pick.is_some(),
            "the MLP should pick a family for valid features"
        );
        assert!(
            cands.contains(&pick.unwrap()),
            "the pick must be one of the candidates"
        );
    }

    #[test]
    fn never_picks_outside_candidates() {
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        let features = [0.7f32; 20];
        // Only PNG offered → PNG or None, never another family.
        let cands = [ImageFormat::Png];
        assert!(
            picker
                .pick(&features, &cands)
                .map_or(true, |f| f == ImageFormat::Png)
        );
    }
}
