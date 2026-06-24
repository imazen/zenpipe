//! Optional MLP-backed [`FormatPicker`] over zenpicker (the `picker` feature).
//!
//! `zenpicker` is codec-agnostic — it returns its own `CodecFamily` from an MLP
//! over zenanalyze image features. This adapter maps a zencodecs candidate set to
//! the picker's allowed-family mask, runs the meta-picker, and maps the chosen
//! family back to an [`ImageFormat`], so it plugs straight into
//! [`select_format_from_intent_with_picker`](crate::select_format_from_intent_with_picker).
//!
//! Three entry points, in ascending capability:
//!
//! - [`MlpFormatPicker::pick`] (the [`FormatPicker`] impl) — re-rank the
//!   candidates zencodecs already proved valid, from a caller-supplied feature
//!   vector. It can only ever choose among the offered formats; it never widens
//!   the allowed set.
//! - [`MlpFormatPicker::pick_with_budget`] — the same, plus a per-candidate
//!   additive score penalty, so a format that is *feasible but degraded* under an
//!   encode-resource budget (e.g. JXL only affordable at a cheaper effort than it
//!   was trained at) can lose the argmin to a rival that runs at full effort.
//!   Hard-infeasible formats are expressed by *omission* — drop them from
//!   `candidates`.
//! - [`MlpFormatPicker::pick_from_offer`] (the `picker-api` feature) — negotiate
//!   feature *reuse* against a [`zenanalyze_api::Offer`] using the model's
//!   declared feature columns, so one zenanalyze pass can feed this meta-picker
//!   and every per-codec picker without re-extracting. Falls back to
//!   [`OfferPick::NeedsAnalysis`] when the offer can't satisfy the model.
//!
//! The feature vector is the caller's to produce (e.g. via zenanalyze) — the base
//! `picker` feature takes no analysis dependency; only `picker-api` pulls the
//! zero-dep `zenanalyze-api` contract crate.

use crate::select::FormatPicker;
use crate::{CodecError, Result};
use alloc::boxed::Box;
use zencodec::ImageFormat;
use zenpicker::{AllowedFamilies, CodecFamily, MetaPicker};
use zenpredict::{AllowedMask, Model};

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

/// The allowed-family mask for a candidate set: every candidate whose format maps
/// to a picker family is allowed, the rest are dropped. The picker can only ever
/// choose among the offered families — it never widens the set.
fn allowed_from(candidates: &[ImageFormat]) -> AllowedFamilies {
    let mut allowed = AllowedFamilies::none();
    for &fmt in candidates {
        if let Some(fam) = family_of(fmt) {
            allowed = allowed.allow(fam);
        }
    }
    allowed
}

/// Map a chosen family back to a candidate `ImageFormat`, but only if it is
/// actually one of `candidates` — defensive so the picker can never return a
/// format the caller didn't offer.
fn to_candidate(family: CodecFamily, candidates: &[ImageFormat]) -> Option<ImageFormat> {
    let chosen = format_of(family)?;
    candidates.iter().copied().find(|&c| c == chosen)
}

/// A [`FormatPicker`] backed by a zenpicker meta-model (an MLP over zenanalyze
/// features).
///
/// Construct once from baked model bytes, then either pass `&self` to
/// [`select_format_from_intent_with_picker`](crate::select_format_from_intent_with_picker)
/// (via the [`FormatPicker`] trait) or call the inherent
/// [`pick_with_budget`](Self::pick_with_budget) /
/// [`pick_from_offer`](Self::pick_from_offer) methods for the richer routes.
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

    /// Budget-aware pick: like [`pick`](FormatPicker::pick) but each candidate
    /// carries an additive **score penalty** applied in the model's argmin space
    /// (lower score wins), so a feasible-but-degraded format can lose to a rival
    /// that runs at full effort.
    ///
    /// `penalties[k]` is the penalty for `candidates[k]` (the two slices align
    /// 1:1; a shorter `penalties` leaves the unmatched candidates un-penalised).
    /// The penalty is the caller's **degradation cost** — typically
    /// `RD(effort achievable under the budget) − RD(reference effort the model
    /// was trained at)` for that family, computed by the budget layer (e.g.
    /// zencodecs [`estimate`](crate::estimate)). `0.0` means "no degradation"; a
    /// larger value pushes the family down the ranking; prefer *omitting* a
    /// candidate over an infinite penalty for a hard-infeasible format.
    ///
    /// Returns `None` when no candidate maps to a picker family, the model errors
    /// (shape mismatch / NaN), or the winning family isn't among `candidates`.
    ///
    /// Reduces exactly to [`pick`](FormatPicker::pick) when every penalty is
    /// `0.0`.
    pub fn pick_with_budget(
        &self,
        features: &[f32],
        candidates: &[ImageFormat],
        penalties: &[f32],
    ) -> Option<ImageFormat> {
        debug_assert_eq!(
            candidates.len(),
            penalties.len(),
            "pick_with_budget: penalties must align 1:1 with candidates"
        );
        // Allowed-family mask + per-family penalty, indexed by `CodecFamily`'s
        // stable index — which equals the model's output index, by the
        // family-order contract `validate_family_order` checks.
        let mut allowed = AllowedFamilies::none();
        let mut penalty_by_family = [0.0f32; CodecFamily::COUNT];
        for (&fmt, &pen) in candidates.iter().zip(penalties) {
            if let Some(fam) = family_of(fmt) {
                allowed = allowed.allow(fam);
                penalty_by_family[fam.index()] += pen;
            }
        }
        if !allowed.any() {
            return None;
        }
        let mut picker = MetaPicker::new(&self.model);
        let mask = AllowedMask::new(allowed.as_slice());
        // Identity score space (the meta-picker's outputs are already in the
        // argmin-target space, matching `MetaPicker::pick`) plus the per-family
        // budget penalty. `i` is the output / family index.
        let idx = picker
            .predictor()
            .argmin_masked_with_scorer(features, &mask, |out, i| {
                out[i] + penalty_by_family.get(i).copied().unwrap_or(0.0)
            })
            .ok()??;
        to_candidate(CodecFamily::ALL[idx], candidates)
    }

    /// Pick from an existing [`zenanalyze_api::Offer`] by negotiating feature
    /// **reuse** against the model's declared feature columns — so a single
    /// zenanalyze pass can feed this meta-picker *and* every per-codec picker
    /// without re-extracting.
    ///
    /// The model declares which columns it consumes (qualified `name@hash`
    /// identities); [`Offer::reuse_for`](zenanalyze_api::Offer::reuse_for)
    /// returns the values in the model's order iff the offer carries every one at
    /// the matching code version. The outcomes:
    ///
    /// - [`OfferPick::Picked`] — the offer satisfied the model and the MLP chose
    ///   this format (always one of `candidates`).
    /// - [`OfferPick::NoCandidate`] — features were reused, but no offered
    ///   candidate won (empty `candidates`, or the winning family wasn't offered).
    /// - [`OfferPick::NeedsAnalysis`] — the offer can't satisfy the model
    ///   (feature drift, a missing column, or a pre-`name@hash` bake that declares
    ///   no `wants`). Run an own zenanalyze pass and call
    ///   [`pick`](FormatPicker::pick).
    ///
    /// Requires the `picker-api` feature.
    #[cfg(feature = "picker-api")]
    pub fn pick_from_offer(
        &self,
        offer: &zenanalyze_api::Offer<'_>,
        candidates: &[ImageFormat],
    ) -> OfferPick {
        let allowed = allowed_from(candidates);
        if !allowed.any() {
            return OfferPick::NoCandidate;
        }
        let mut picker = MetaPicker::new(&self.model);
        // Negotiate features from the offer. The `Request` borrows `picker`, so
        // scope it: it must drop before the `&mut self` `pick` below.
        let features = {
            let Some(req) = picker.feature_request() else {
                // No declared wants (a pre-`name@hash` bake) — can't reuse.
                return OfferPick::NeedsAnalysis;
            };
            match offer.reuse_for(&req) {
                Some(values) => values,
                None => return OfferPick::NeedsAnalysis,
            }
        };
        match picker.pick(&features, &allowed) {
            Ok(Some(family)) => {
                to_candidate(family, candidates).map_or(OfferPick::NoCandidate, OfferPick::Picked)
            }
            Ok(None) => OfferPick::NoCandidate,
            // A runtime error (shape / NaN) is recoverable by re-running analysis.
            Err(_) => OfferPick::NeedsAnalysis,
        }
    }
}

/// The outcome of [`MlpFormatPicker::pick_from_offer`] — a feature-reuse
/// negotiation can succeed with a pick, succeed with no eligible candidate, or
/// fail to reuse (the caller must run its own analysis pass).
#[cfg(feature = "picker-api")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfferPick {
    /// The offer satisfied the model and the MLP chose this format (one of the
    /// candidates passed in).
    Picked(ImageFormat),
    /// Features were reused, but no offered candidate won the argmin (empty
    /// `candidates`, or the chosen family wasn't among them).
    NoCandidate,
    /// The offer could not satisfy the model (feature drift, a missing column, or
    /// a pre-`name@hash` bake with no declared `wants`). Run an own zenanalyze
    /// pass and call [`MlpFormatPicker::pick`](FormatPicker::pick).
    NeedsAnalysis,
}

impl FormatPicker for MlpFormatPicker {
    fn pick(&self, features: &[f32], candidates: &[ImageFormat]) -> Option<ImageFormat> {
        // Candidates → allowed-family mask. Formats with no picker family are
        // dropped; the picker can only choose among the offered families.
        let allowed = allowed_from(candidates);
        if !allowed.any() {
            return None;
        }
        // A fresh picker over the owned model — one selection per image, so the
        // scratch allocation is negligible against the encode it informs.
        let mut picker = MetaPicker::new(&self.model);
        let family = picker.pick(features, &allowed).ok()??;
        to_candidate(family, candidates)
    }

    /// Budget-aware override: folds the per-candidate degradation penalties into
    /// the MLP's argmin via [`pick_with_budget`](Self::pick_with_budget), so the
    /// budget seam ([`select_format_with_budget_picker`](crate::select_format_with_budget_picker))
    /// re-ranks with resource costs rather than ignoring them.
    fn pick_with_penalties(
        &self,
        features: &[f32],
        candidates: &[ImageFormat],
        penalties: &[f32],
    ) -> Option<ImageFormat> {
        self.pick_with_budget(features, candidates, penalties)
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
                .is_none_or(|f| f == ImageFormat::Png)
        );
    }

    #[test]
    fn budget_zero_penalties_match_plain_pick() {
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        let features = [0.42f32; 20];
        let cands = [
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Jxl,
            ImageFormat::Avif,
        ];
        let plain = picker.pick(&features, &cands);
        let budgeted = picker.pick_with_budget(&features, &cands, &[0.0; 4]);
        assert_eq!(
            plain, budgeted,
            "zero penalties must reduce to a plain pick"
        );
        assert!(budgeted.is_some());
    }

    #[test]
    fn budget_penalty_reranks_away_from_natural_winner() {
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        let features = [0.55f32; 20];
        let cands = [
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Jxl,
            ImageFormat::Avif,
        ];
        let winner = picker.pick(&features, &cands).expect("a natural winner");
        // Slam a huge penalty on whichever format the model naturally chose; with
        // ≥2 candidates the argmin must move to a different one.
        let mut penalties = [0.0f32; 4];
        for (k, &c) in cands.iter().enumerate() {
            if c == winner {
                penalties[k] = 1.0e6;
            }
        }
        let reranked = picker.pick_with_budget(&features, &cands, &penalties);
        assert!(
            reranked.is_some(),
            "another candidate should win once the leader is penalised"
        );
        assert_ne!(reranked, Some(winner), "the penalised leader must not win");
        assert!(cands.contains(&reranked.unwrap()));
    }

    #[test]
    fn budget_no_family_candidates_is_none() {
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        // No candidates at all → nothing to pick.
        assert!(picker.pick_with_budget(&[0.1f32; 20], &[], &[]).is_none());
    }

    #[cfg(feature = "picker-api")]
    #[test]
    fn offer_without_wants_needs_analysis() {
        use zenanalyze_api::{Offer, Provenance};
        let picker = MlpFormatPicker::from_model_bytes(MODEL).unwrap();
        // The vendored v0.5 bake predates the `name@hash` column convention, so it
        // declares no `wants` → no offer can satisfy it → NeedsAnalysis. (A model
        // baked with qualified columns exercises the reuse / Picked path.)
        let offer = Offer::new(&[], Provenance::new("test"));
        let cands = [ImageFormat::Jpeg, ImageFormat::WebP];
        assert_eq!(
            picker.pick_from_offer(&offer, &cands),
            OfferPick::NeedsAnalysis
        );
    }
}
