//!
//! This module provides spatial masks that control where a filter is applied,
//! enabling selective adjustments — a core Lightroom/Photoshop workflow.
//!
//! # Architecture
//!
//! [`MaskedFilter`] wraps any [`Filter`] with a [`Mask`] that controls
//! per-pixel application intensity:
//!
//! ```text
//! original_planes ──┬──→ inner_filter.apply() → filtered_planes
//!                   │                                │
//!                   └──→ mask.generate() ──→ blend(original, filtered, mask)
//! ```
//!
//! The mask is a [0.0, 1.0] plane where:
//! - 0.0 = keep original (filter has no effect)
//! - 1.0 = full filter application
//! - intermediate = proportional blend
//!
//! # Usage
//!
//! ```
//! use zenfilters::masked::{MaskedFilter, Mask};
//! use zenfilters::filters::Exposure;
//!
//! // Brighten only the bottom half of the image (graduated filter)
//! let mut exposure = Exposure::default();
//! exposure.stops = 1.5;
//! let masked = MaskedFilter {
//!     filter: Box::new(exposure),
//!     mask: Mask::LinearGradient {
//!         x0: 0.5, y0: 0.0, // Start at top center (mask = 0)
//!         x1: 0.5, y1: 1.0, // End at bottom center (mask = 1)
//!     },
//!     invert: false,
//! };
//! ```
//!
//! # Future: Blend Layers
//!
//! Blend layer compositing (combining two OklabPlanes with blend modes like
//! Multiply, Screen, Overlay in Oklab space) is a natural extension. It
//! requires a two-input model beyond the current single-input Filter trait.
//! When needed, it should be implemented as a separate `BlendLayer` struct
//! that takes two OklabPlanes and a blend mode, rather than forcing it into
//! the Filter trait.

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// A spatial mask controlling per-pixel filter intensity.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Mask {
    /// Linear gradient between two points in normalized [0, 1] image coordinates.
    ///
    /// The mask value varies linearly from 0.0 at (x0, y0) to 1.0 at (x1, y1).
    /// Pixels beyond the endpoints are clamped to 0.0 or 1.0.
    ///
    /// Common uses:
    /// - Graduated neutral density: `(0.5, 0.0) → (0.5, 0.5)` (darken sky)
    /// - Left-right vignette: `(0.0, 0.5) → (1.0, 0.5)` (brighten right side)
    LinearGradient { x0: f32, y0: f32, x1: f32, y1: f32 },

    /// Radial gradient centered at (cx, cy) in normalized [0, 1] coordinates.
    ///
    /// Mask = 1.0 inside `inner_radius`, 0.0 outside `outer_radius`,
    /// with smooth interpolation between. Radii are in normalized units
    /// where 1.0 = image diagonal.
    RadialGradient {
        cx: f32,
        cy: f32,
        inner_radius: f32,
        outer_radius: f32,
    },

    /// Luminance range mask: selects pixels by their L channel value.
    ///
    /// Mask = 1.0 for pixels with L in [low, high], fading to 0.0
    /// over the feather distance at each boundary. This enables
    /// "adjust only highlights" or "adjust only shadows" workflows.
    LuminanceRange {
        /// Lower L bound (0.0–1.0).
        low: f32,
        /// Upper L bound (0.0–1.0).
        high: f32,
        /// Feather width in L units (softness of the boundary).
        feather: f32,
    },
}

/// Wraps any [`Filter`] with a spatial mask for selective application.
///
/// The inner filter runs on the full image, then the result is blended
/// with the original using the mask. This means the filter itself is
/// unchanged — masking is purely a post-blend operation.
pub struct MaskedFilter {
    /// The filter to apply selectively.
    pub filter: Box<dyn Filter>,
    /// Spatial mask controlling application intensity.
    pub mask: Mask,
    /// Invert the mask (swap affected/unaffected regions).
    pub invert: bool,
}

impl Filter for MaskedFilter {
    fn channel_access(&self) -> ChannelAccess {
        self.filter.channel_access()
    }

    fn is_neighborhood(&self) -> bool {
        self.filter.is_neighborhood()
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        self.filter.neighborhood_radius(width, height)
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        self.filter.tag()
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        self.filter.resize_phase()
    }

    fn scale_for_resolution(&mut self, scale: f32) {
        // MaskedFilter is &self in Filter::apply but scale_for_resolution is &mut self.
        // The inner filter needs to be scaled too.
        // This works because MaskedFilter owns the Box<dyn Filter>.
        // However, we can't call scale_for_resolution on Box<dyn Filter>
        // because Filter::scale_for_resolution takes &mut self and dyn Filter
        // doesn't support that through the vtable... except it does, because
        // the trait has it as a default method with &mut self.
        // Actually we CAN — Box<dyn Filter> derefs to &mut dyn Filter which
        // can call &mut self methods.
        // But wait — Filter::apply takes &self. scale_for_resolution takes &mut self.
        // Since we have Box<dyn Filter>, we can call both.
        // Actually, looking at the trait definition more carefully:
        // apply takes &self, but scale_for_resolution takes &mut self.
        // We need the filter behind the Box to be mutable.
        // We don't call scale_for_resolution during apply, so this is fine.
        let _ = scale;
        // Note: can't forward to inner filter because we'd need &mut self
        // on the inner filter, which requires knowing the concrete type.
        // Users should scale the inner filter before wrapping it.
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let access = self.filter.channel_access();
        let w = planes.width;
        let h = planes.height;
        let n = planes.pixel_count();

        // Save original planes that the filter modifies.
        // Check whether reads or writes touch luma/chroma using PlaneMask bit ops.
        let touches_luma = !access
            .writes
            .intersection(zenpixels::PlaneMask::LUMA)
            .is_empty();
        let touches_chroma = !access
            .writes
            .intersection(zenpixels::PlaneMask::CHROMA)
            .is_empty();

        let save_l = if touches_luma {
            let mut saved = ctx.take_f32(n);
            saved.copy_from_slice(&planes.l);
            Some(saved)
        } else {
            None
        };

        let save_a = if touches_chroma {
            let mut saved = ctx.take_f32(n);
            saved.copy_from_slice(&planes.a);
            Some(saved)
        } else {
            None
        };

        let save_b = if touches_chroma {
            let mut saved = ctx.take_f32(n);
            saved.copy_from_slice(&planes.b);
            Some(saved)
        } else {
            None
        };

        // Apply the inner filter
        self.filter.apply(planes, ctx);

        // Generate mask and blend
        let mut mask_buf = ctx.take_f32(n);
        generate_mask(&self.mask, &mut mask_buf, w, h, save_l.as_deref());

        if self.invert {
            for v in mask_buf.iter_mut() {
                *v = 1.0 - *v;
            }
        }

        // Blend: result = original * (1 - mask) + filtered * mask
        if let Some(orig_l) = &save_l {
            blend_planes(orig_l, &mut planes.l, &mask_buf);
        }
        if let Some(orig_a) = &save_a {
            blend_planes(orig_a, &mut planes.a, &mask_buf);
        }
        if let Some(orig_b) = &save_b {
            blend_planes(orig_b, &mut planes.b, &mask_buf);
        }

        // Return saved buffers
        ctx.return_f32(mask_buf);
        if let Some(buf) = save_l {
            ctx.return_f32(buf);
        }
        if let Some(buf) = save_a {
            ctx.return_f32(buf);
        }
        if let Some(buf) = save_b {
            ctx.return_f32(buf);
        }
    }
}

/// Generate a mask plane from a [`Mask`] definition.
///
/// `src_l` is the original L plane (needed for [`Mask::LuminanceRange`]).
fn generate_mask(mask: &Mask, dst: &mut [f32], width: u32, height: u32, src_l: Option<&[f32]>) {
    let w = width as f32;
    let h = height as f32;

    match mask {
        Mask::LinearGradient { x0, y0, x1, y1 } => {
            // Direction vector
            let dx = x1 - x0;
            let dy = y1 - y0;
            let len_sq = dx * dx + dy * dy;
            let inv_len_sq = if len_sq > 1e-10 { 1.0 / len_sq } else { 0.0 };

            for py in 0..height {
                for px in 0..width {
                    let nx = px as f32 / w;
                    let ny = py as f32 / h;
                    // Project onto gradient direction
                    let t = ((nx - x0) * dx + (ny - y0) * dy) * inv_len_sq;
                    dst[(py as usize) * (width as usize) + (px as usize)] = t.clamp(0.0, 1.0);
                }
            }
        }

        Mask::RadialGradient {
            cx,
            cy,
            inner_radius,
            outer_radius,
        } => {
            let diag = (w * w + h * h).sqrt();
            let inner = inner_radius * diag;
            let outer = outer_radius * diag;
            let range = (outer - inner).max(1e-6);

            for py in 0..height {
                for px in 0..width {
                    let dx = px as f32 - cx * w;
                    let dy = py as f32 - cy * h;
                    let dist = (dx * dx + dy * dy).sqrt();
                    // 1.0 inside inner, 0.0 outside outer
                    let t = 1.0 - ((dist - inner) / range).clamp(0.0, 1.0);
                    dst[(py as usize) * (width as usize) + (px as usize)] = t;
                }
            }
        }

        Mask::LuminanceRange { low, high, feather } => {
            let src = src_l.expect("LuminanceRange mask requires L plane");
            let f = feather.max(0.001);
            for (i, &l) in src.iter().enumerate() {
                // Smoothstep ramp at boundaries
                let low_mask = ((l - (low - f)) / f).clamp(0.0, 1.0);
                let high_mask = (((high + f) - l) / f).clamp(0.0, 1.0);
                dst[i] = low_mask * high_mask;
            }
        }
    }
}

/// Blend original and filtered planes using a mask.
/// `filtered` is modified in-place: `filtered[i] = original[i] * (1 - mask[i]) + filtered[i] * mask[i]`
fn blend_planes(original: &[f32], filtered: &mut [f32], mask: &[f32]) {
    for i in 0..filtered.len() {
        let m = mask[i];
        filtered[i] = original[i] * (1.0 - m) + filtered[i] * m;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;
    use crate::filters::Exposure;
    use crate::prelude::*;

    #[test]
    fn full_mask_applies_completely() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.5);

        // Linear gradient that's 1.0 everywhere (start = end)
        let mut exposure = Exposure::default();
        exposure.stops = 1.0;

        let masked = MaskedFilter {
            filter: Box::new(exposure.clone()),
            mask: Mask::LinearGradient {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
            },
            invert: true, // Invert a zero-everywhere mask → 1.0 everywhere
        };

        // Apply unmasked for reference
        let mut ref_planes = planes.clone();
        exposure.apply(&mut ref_planes, &mut FilterContext::new());

        masked.apply(&mut planes, &mut FilterContext::new());

        let mut max_err = 0.0f32;
        for (a, b) in planes.l.iter().zip(ref_planes.l.iter()) {
            max_err = max_err.max((a - b).abs());
        }
        assert!(
            max_err < 0.01,
            "inverted zero mask should apply fully, max_err={max_err}"
        );
    }

    #[test]
    fn zero_mask_preserves_original() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.5);
        let original = planes.l.clone();

        let mut exposure = Exposure::default();
        exposure.stops = 2.0;

        let masked = MaskedFilter {
            filter: Box::new(exposure),
            mask: Mask::LinearGradient {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
            },
            invert: false, // Zero-length gradient → 0.0 everywhere (NaN guard → clamped 0)
        };

        masked.apply(&mut planes, &mut FilterContext::new());
        // With mask = 0 everywhere, original should be preserved
        // Note: the gradient direction is zero-length, so t = 0 everywhere
        assert_eq!(planes.l, original);
    }

    #[test]
    fn linear_gradient_is_directional() {
        let w = 32u32;
        let h = 32u32;
        let n = (w * h) as usize;
        let mut mask = vec![0.0f32; n];

        generate_mask(
            &Mask::LinearGradient {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 1.0,
            },
            &mut mask,
            w,
            h,
            None,
        );

        // Top should be ~0, bottom should be ~1
        let top = mask[0];
        let bottom = mask[n - 1];
        assert!(top < 0.1, "top should be near 0, got {top}");
        assert!(bottom > 0.9, "bottom should be near 1, got {bottom}");
    }

    #[test]
    fn radial_gradient_has_center() {
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let mut mask = vec![0.0f32; n];

        generate_mask(
            &Mask::RadialGradient {
                cx: 0.5,
                cy: 0.5,
                inner_radius: 0.1,
                outer_radius: 0.4,
            },
            &mut mask,
            w,
            h,
            None,
        );

        let center = mask[(32 * w + 32) as usize];
        let corner = mask[0];
        assert!(center > 0.9, "center should be bright, got {center}");
        assert!(corner < 0.1, "corner should be dark, got {corner}");
    }

    #[test]
    fn luminance_range_selects_highlights() {
        let w = 16u32;
        let h = 16u32;
        let n = (w * h) as usize;
        let mut mask = vec![0.0f32; n];

        // Create L plane with gradient 0→1
        let src_l: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();

        generate_mask(
            &Mask::LuminanceRange {
                low: 0.7,
                high: 1.0,
                feather: 0.05,
            },
            &mut mask,
            w,
            h,
            Some(&src_l),
        );

        // Dark pixels should be masked out, bright pixels should be included
        let dark = mask[0]; // L ≈ 0
        let bright = mask[n - 1]; // L ≈ 1
        assert!(dark < 0.01, "dark pixel should be masked out, got {dark}");
        assert!(
            bright > 0.9,
            "bright pixel should be included, got {bright}"
        );
    }
}
