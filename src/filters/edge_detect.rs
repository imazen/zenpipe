use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Edge detection mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EdgeMode {
    /// Sobel operator: directional 3×3 gradient (horizontal + vertical).
    /// Magnitude = sqrt(Gx² + Gy²). Good for edge detection and contours.
    Sobel,
    /// Laplacian operator: isotropic second-derivative 3×3 kernel.
    /// Detects edges and fine texture. More sensitive to noise than Sobel.
    Laplacian,
}

/// Edge detection on the L (lightness) channel.
///
/// Replaces L with gradient magnitude (Sobel) or second derivative (Laplacian),
/// normalized to [0, 1]. Chroma channels are zeroed to produce a grayscale
/// edge map.
///
/// Use cases:
/// - Mask generation for selective adjustments (edge-aware masks)
/// - Document analysis (text boundary detection, layout)
/// - Input for downstream algorithms (corner detection, line fitting)
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct EdgeDetect {
    /// Detection algorithm.
    pub mode: EdgeMode,
    /// Output scaling factor. Higher values amplify weak edges.
    /// Default: 1.0. Range: 0.1–5.0.
    pub strength: f32,
}

impl Default for EdgeDetect {
    fn default() -> Self {
        Self {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
    }
}

impl Filter for EdgeDetect {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        1 // Both Sobel and Laplacian use 3×3 kernels
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::EdgeDetect
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PreResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let w = planes.width as usize;
        let h = planes.height as usize;
        let n = w * h;

        let mut dst = ctx.take_f32(n);

        match self.mode {
            EdgeMode::Sobel => sobel(&planes.l, &mut dst, w, h, self.strength),
            EdgeMode::Laplacian => laplacian(&planes.l, &mut dst, w, h, self.strength),
        }

        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);

        // Zero chroma to produce grayscale edge map
        planes.a.fill(0.0);
        planes.b.fill(0.0);
    }
}

/// Sobel edge detection: gradient magnitude from 3×3 directional kernels.
///
/// Gx = [-1  0  1]    Gy = [-1 -2 -1]
///      [-2  0  2]         [ 0  0  0]
///      [-1  0  1]         [ 1  2  1]
fn sobel(src: &[f32], dst: &mut [f32], w: usize, h: usize, strength: f32) {
    for y in 0..h {
        for x in 0..w {
            // Sample 3×3 neighborhood with edge clamping
            let p = |dx: isize, dy: isize| -> f32 {
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                src[sy * w + sx]
            };

            let tl = p(-1, -1);
            let tc = p(0, -1);
            let tr = p(1, -1);
            let ml = p(-1, 0);
            let mr = p(1, 0);
            let bl = p(-1, 1);
            let bc = p(0, 1);
            let br = p(1, 1);

            let gx = -tl + tr - 2.0 * ml + 2.0 * mr - bl + br;
            let gy = -tl - 2.0 * tc - tr + bl + 2.0 * bc + br;

            let mag = (gx * gx + gy * gy).sqrt() * strength;
            dst[y * w + x] = mag.clamp(0.0, 1.0);
        }
    }
}

/// Laplacian edge detection: isotropic 3×3 second-derivative kernel.
///
/// Kernel = [ 0 -1  0]
///          [-1  4 -1]
///          [ 0 -1  0]
fn laplacian(src: &[f32], dst: &mut [f32], w: usize, h: usize, strength: f32) {
    for y in 0..h {
        for x in 0..w {
            let p = |dx: isize, dy: isize| -> f32 {
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                src[sy * w + sx]
            };

            let center = p(0, 0);
            let lap = -p(0, -1) - p(-1, 0) + 4.0 * center - p(1, 0) - p(0, 1);

            // Take absolute value (edges are both positive and negative crossings)
            let mag = lap.abs() * strength;
            dst[y * w + x] = mag.clamp(0.0, 1.0);
        }
    }
}

static EDGE_DETECT_SCHEMA: FilterSchema = FilterSchema {
    name: "edge_detect",
    label: "Edge Detect",
    description: "Sobel/Laplacian edge detection on L channel (produces grayscale edge map)",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "mode",
            label: "Mode",
            description: "Sobel (directional gradient) or Laplacian (second derivative)",
            kind: ParamKind::Int {
                min: 0,
                max: 1,
                default: 0,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Output scaling (amplifies weak edges)",
            kind: ParamKind::Float {
                min: 0.1,
                max: 5.0,
                default: 1.0,
                identity: 1.0,
                step: 0.1,
            },
            unit: "×",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for EdgeDetect {
    fn schema() -> &'static FilterSchema {
        &EDGE_DETECT_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "mode" => Some(ParamValue::Int(match self.mode {
                EdgeMode::Sobel => 0,
                EdgeMode::Laplacian => 1,
            })),
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "mode" => {
                if let Some(v) = value.as_i32() {
                    self.mode = match v {
                        0 => EdgeMode::Sobel,
                        _ => EdgeMode::Laplacian,
                    };
                    true
                } else {
                    false
                }
            }
            "strength" => {
                if let Some(v) = value.as_f32() {
                    self.strength = v;
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
    fn constant_plane_has_no_edges() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!(v.abs() < 1e-6, "constant plane should have zero edges, got {v}");
        }
    }

    #[test]
    fn detects_step_edge_sobel() {
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Pixels at the edge (x=15,16) should have high gradient
        let edge = planes.l[planes.index(15, 16)];
        // Interior pixels should have zero gradient
        let interior = planes.l[planes.index(8, 16)];
        assert!(edge > 0.1, "edge pixel should have high gradient, got {edge}");
        assert!(
            interior < 0.01,
            "interior pixel should have near-zero gradient, got {interior}"
        );
    }

    #[test]
    fn detects_step_edge_laplacian() {
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Laplacian,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let edge = planes.l[planes.index(15, 16)];
        let interior = planes.l[planes.index(8, 16)];
        assert!(edge > 0.1, "edge pixel should be detected, got {edge}");
        assert!(
            interior < 0.01,
            "interior pixel should be near-zero, got {interior}"
        );
    }

    #[test]
    fn chroma_is_zeroed() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.a.fill(0.1);
        planes.b.fill(-0.05);
        EdgeDetect::default().apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.a {
            assert!(v.abs() < 1e-6, "a should be zeroed, got {v}");
        }
        for &v in &planes.b {
            assert!(v.abs() < 1e-6, "b should be zeroed, got {v}");
        }
    }

    #[test]
    fn strength_amplifies_edges() {
        let mut planes_weak = OklabPlanes::new(32, 32);
        let mut planes_strong = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes_weak.index(x, y);
                let v = if x < 16 { 0.4 } else { 0.6 };
                planes_weak.l[i] = v;
                planes_strong.l[i] = v;
            }
        }

        let mut ctx = FilterContext::new();
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes_weak, &mut ctx);
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 3.0,
        }
        .apply(&mut planes_strong, &mut ctx);

        let weak_edge = planes_weak.l[planes_weak.index(15, 16)];
        let strong_edge = planes_strong.l[planes_strong.index(15, 16)];
        assert!(
            strong_edge > weak_edge,
            "strength should amplify: weak={weak_edge}, strong={strong_edge}"
        );
    }
}
