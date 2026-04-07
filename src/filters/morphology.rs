use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Morphological operation type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MorphOp {
    /// Shrink bright regions (local minimum). Removes small bright noise.
    Erode,
    /// Expand bright regions (local maximum). Fills small dark holes.
    Dilate,
    /// Erode then dilate. Removes small bright features, smooths boundaries.
    Open,
    /// Dilate then erode. Fills small dark gaps, connects nearby bright regions.
    Close,
    /// Source minus opened. Extracts small bright details.
    TopHat,
    /// Closed minus source. Extracts small dark details.
    BlackHat,
}

/// Morphological filter for binary/grayscale operations.
///
/// Applies erosion, dilation, opening, or closing to the L channel
/// using a square structuring element. Useful for document processing,
/// noise removal, and mask cleanup.
///
/// In Oklab space, these operations affect perceived brightness:
/// - **Erode**: darkens by local minimum — removes bright specks
/// - **Dilate**: brightens by local maximum — fills dark holes
/// - **Open** (erode→dilate): removes small bright noise
/// - **Close** (dilate→erode): fills small dark gaps
/// - **TopHat**: source minus opened — extracts small bright features
/// - **BlackHat**: closed minus source — extracts small dark features
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Morphology {
    /// The morphological operation.
    pub op: MorphOp,
    /// Structuring element radius (1 = 3×3, 2 = 5×5, 3 = 7×7).
    pub radius: u32,
    /// Whether to also process chroma channels.
    pub process_chroma: bool,
}

impl Default for Morphology {
    fn default() -> Self {
        Self {
            op: MorphOp::Dilate,
            radius: 1,
            process_chroma: false,
        }
    }
}

impl Filter for Morphology {
    fn channel_access(&self) -> ChannelAccess {
        if self.process_chroma {
            ChannelAccess::L_AND_CHROMA
        } else {
            ChannelAccess::L_ONLY
        }
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        match self.op {
            MorphOp::Open | MorphOp::Close | MorphOp::TopHat | MorphOp::BlackHat => self.radius * 2,
            _ => self.radius,
        }
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PreResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.radius == 0 {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;
        let r = self.radius as usize;

        match self.op {
            MorphOp::Erode => {
                morph_plane(&mut planes.l, ctx, w, h, r, false);
                if self.process_chroma {
                    morph_plane(&mut planes.a, ctx, w, h, r, false);
                    morph_plane(&mut planes.b, ctx, w, h, r, false);
                }
            }
            MorphOp::Dilate => {
                morph_plane(&mut planes.l, ctx, w, h, r, true);
                if self.process_chroma {
                    morph_plane(&mut planes.a, ctx, w, h, r, true);
                    morph_plane(&mut planes.b, ctx, w, h, r, true);
                }
            }
            MorphOp::Open => {
                // Erode then dilate
                morph_plane(&mut planes.l, ctx, w, h, r, false);
                morph_plane(&mut planes.l, ctx, w, h, r, true);
                if self.process_chroma {
                    morph_plane(&mut planes.a, ctx, w, h, r, false);
                    morph_plane(&mut planes.a, ctx, w, h, r, true);
                    morph_plane(&mut planes.b, ctx, w, h, r, false);
                    morph_plane(&mut planes.b, ctx, w, h, r, true);
                }
            }
            MorphOp::Close => {
                // Dilate then erode
                morph_plane(&mut planes.l, ctx, w, h, r, true);
                morph_plane(&mut planes.l, ctx, w, h, r, false);
                if self.process_chroma {
                    morph_plane(&mut planes.a, ctx, w, h, r, true);
                    morph_plane(&mut planes.a, ctx, w, h, r, false);
                    morph_plane(&mut planes.b, ctx, w, h, r, true);
                    morph_plane(&mut planes.b, ctx, w, h, r, false);
                }
            }
            MorphOp::TopHat => {
                // TopHat = source - opened
                let original = planes.l.clone();
                morph_plane(&mut planes.l, ctx, w, h, r, false);
                morph_plane(&mut planes.l, ctx, w, h, r, true);
                for (o, s) in planes.l.iter_mut().zip(original.iter()) {
                    *o = (*s - *o).max(0.0);
                }
            }
            MorphOp::BlackHat => {
                // BlackHat = closed - source
                let original = planes.l.clone();
                morph_plane(&mut planes.l, ctx, w, h, r, true);
                morph_plane(&mut planes.l, ctx, w, h, r, false);
                for (c, s) in planes.l.iter_mut().zip(original.iter()) {
                    *c = (*c - *s).max(0.0);
                }
            }
        }
    }
}

/// Apply a single erosion (max=false) or dilation (max=true) to a plane.
fn morph_plane(
    plane: &mut alloc::vec::Vec<f32>,
    ctx: &mut FilterContext,
    w: usize,
    h: usize,
    radius: usize,
    is_max: bool,
) {
    let n = w * h;
    let mut dst = ctx.take_f32(n);

    for y in 0..h {
        for x in 0..w {
            let mut val = if is_max {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            };

            for ky in 0..2 * radius + 1 {
                for kx in 0..2 * radius + 1 {
                    let sy = (y + ky).saturating_sub(radius).min(h - 1);
                    let sx = (x + kx).saturating_sub(radius).min(w - 1);
                    let sample = plane[sy * w + sx];
                    if is_max {
                        if sample > val {
                            val = sample;
                        }
                    } else if sample < val {
                        val = sample;
                    }
                }
            }

            dst[y * w + x] = val;
        }
    }

    let old = core::mem::replace(plane, dst);
    ctx.return_f32(old);
}

// ─── Param schema ──────────────────────────────────────────────────

static MORPHOLOGY_SCHEMA: FilterSchema = FilterSchema {
    name: "morphology",
    label: "Morphology",
    description: "Morphological operations (erode, dilate, open, close)",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "op",
            label: "Operation",
            description: "0=Erode 1=Dilate 2=Open 3=Close 4=TopHat 5=BlackHat",
            kind: ParamKind::Int {
                min: 0,
                max: 5,
                default: 1,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "radius",
            label: "Radius",
            description: "Structuring element radius (1 = 3×3, 2 = 5×5)",
            kind: ParamKind::Int {
                min: 1,
                max: 5,
                default: 1,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "process_chroma",
            label: "Process Chroma",
            description: "Also apply to color channels",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for Morphology {
    fn schema() -> &'static FilterSchema {
        &MORPHOLOGY_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "op" => Some(ParamValue::Int(match self.op {
                MorphOp::Erode => 0,
                MorphOp::Dilate => 1,
                MorphOp::Open => 2,
                MorphOp::Close => 3,
                MorphOp::TopHat => 4,
                MorphOp::BlackHat => 5,
            })),
            "radius" => Some(ParamValue::Int(self.radius as i32)),
            "process_chroma" => Some(ParamValue::Bool(self.process_chroma)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "op" => {
                if let Some(v) = value.as_i32() {
                    self.op = match v {
                        0 => MorphOp::Erode,
                        1 => MorphOp::Dilate,
                        2 => MorphOp::Open,
                        3 => MorphOp::Close,
                        4 => MorphOp::TopHat,
                        _ => MorphOp::BlackHat,
                    };
                    true
                } else {
                    false
                }
            }
            "radius" => {
                if let Some(v) = value.as_i32() {
                    self.radius = (v as u32).clamp(1, 5);
                    true
                } else {
                    false
                }
            }
            "process_chroma" => {
                if let ParamValue::Bool(v) = value {
                    self.process_chroma = v;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn constant_plane_unchanged() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.5);
        Morphology {
            op: MorphOp::Erode,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!((v - 0.5).abs() < 1e-6, "constant should be unchanged");
        }
    }

    #[test]
    fn erode_removes_bright_speck() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.3);
        let idx = planes.index(8, 8);
        planes.l[idx] = 1.0; // bright speck
        Morphology {
            op: MorphOp::Erode,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let speck = planes.l[planes.index(8, 8)];
        assert!(
            (speck - 0.3).abs() < 1e-6,
            "erode should remove bright speck, got {speck}"
        );
    }

    #[test]
    fn dilate_removes_dark_speck() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.7);
        let idx = planes.index(8, 8);
        planes.l[idx] = 0.0; // dark speck
        Morphology {
            op: MorphOp::Dilate,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let speck = planes.l[planes.index(8, 8)];
        assert!(
            (speck - 0.7).abs() < 1e-6,
            "dilate should remove dark speck, got {speck}"
        );
    }

    #[test]
    fn open_removes_bright_noise() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.3);
        // Add a single bright pixel (noise)
        let idx = planes.index(16, 16);
        planes.l[idx] = 1.0;
        // Also add a bright 5×5 block (signal)
        for y in 4..9u32 {
            for x in 4..9u32 {
                let i = planes.index(x, y);
                planes.l[i] = 0.9;
            }
        }
        Morphology {
            op: MorphOp::Open,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Single pixel noise removed
        let noise = planes.l[planes.index(16, 16)];
        assert!(
            (noise - 0.3).abs() < 0.01,
            "open should remove single-pixel noise, got {noise}"
        );
        // Block approximately preserved
        let block = planes.l[planes.index(6, 6)];
        assert!(
            block > 0.7,
            "open should preserve larger structures, got {block}"
        );
    }

    #[test]
    fn close_fills_dark_hole() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.8);
        // Dark single pixel hole
        let idx = planes.index(16, 16);
        planes.l[idx] = 0.0;
        Morphology {
            op: MorphOp::Close,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let hole = planes.l[planes.index(16, 16)];
        assert!(
            (hole - 0.8).abs() < 0.01,
            "close should fill dark hole, got {hole}"
        );
    }

    #[test]
    fn tophat_extracts_small_bright_features() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.3);
        // Small bright dot
        let idx = planes.index(16, 16);
        planes.l[idx] = 1.0;
        Morphology {
            op: MorphOp::TopHat,
            radius: 1,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Background should be ~0 (source - opened ≈ 0.3 - 0.3)
        let bg = planes.l[planes.index(8, 8)];
        assert!(bg < 0.05, "tophat background should be ~0, got {bg}");
        // Bright dot should be detected
        let dot = planes.l[planes.index(16, 16)];
        assert!(dot > 0.3, "tophat should extract bright dot, got {dot}");
    }

    #[test]
    fn zero_radius_is_identity() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 256.0).clamp(0.0, 1.0);
        }
        let orig = planes.l.clone();
        Morphology {
            op: MorphOp::Erode,
            radius: 0,
            process_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }
}
