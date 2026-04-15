use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Lateral chromatic aberration correction.
///
/// Corrects color fringing at image edges caused by lens dispersion.
/// In Oklab, CA manifests as radial displacement of the a (green-red)
/// and b (blue-yellow) planes relative to L. This filter shifts the
/// chroma planes radially to re-align them with luminance.
///
/// Positive values shift the plane outward, negative shifts inward.
/// The shift is fractional relative to the image diagonal:
/// a shift of 0.005 moves edge pixels by ~0.25% of the diagonal (~1.5px at 512px).
/// Typical corrections: ±0.002 to ±0.01. Maximum: ±0.02.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChromaticAberration {
    /// Radial shift for the a (green-red) channel.
    /// Positive = outward, negative = inward. Default: 0.0.
    pub shift_a: f32,
    /// Radial shift for the b (blue-yellow) channel.
    /// Positive = outward, negative = inward. Default: 0.0.
    pub shift_b: f32,
}

impl Default for ChromaticAberration {
    fn default() -> Self {
        Self {
            shift_a: 0.0,
            shift_b: 0.0,
        }
    }
}

impl ChromaticAberration {
    fn is_identity(&self) -> bool {
        self.shift_a.abs() < 1e-7 && self.shift_b.abs() < 1e-7
    }
}

static CHROMATIC_ABERRATION_SCHEMA: FilterSchema = FilterSchema {
    name: "chromatic_aberration",
    label: "Chromatic Aberration",
    description: "Lateral chromatic aberration correction via radial chroma shift",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "shift_a",
            label: "Green-Red Shift",
            description: "Radial shift for the a (green-red) channel",
            kind: ParamKind::Float {
                min: -0.02,
                max: 0.02,
                default: 0.0,
                identity: 0.0,
                step: 0.001,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "shift_b",
            label: "Blue-Yellow Shift",
            description: "Radial shift for the b (blue-yellow) channel",
            kind: ParamKind::Float {
                min: -0.02,
                max: 0.02,
                default: 0.0,
                identity: 0.0,
                step: 0.001,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for ChromaticAberration {
    fn schema() -> &'static FilterSchema {
        &CHROMATIC_ABERRATION_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "shift_a" => Some(ParamValue::Float(self.shift_a)),
            "shift_b" => Some(ParamValue::Float(self.shift_b)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "shift_a" => self.shift_a = v,
            "shift_b" => self.shift_b = v,
            _ => return false,
        }
        true
    }
}

/// Bilinear interpolation sample from a plane at fractional coordinates.
///
/// Bilinear is adequate for CA correction because the shifts are sub-pixel
/// (typically < 0.5px at image edges). Bicubic would add computational cost
/// without visible improvement at these magnitudes.
#[inline]
fn sample_bilinear(plane: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x0 = (x.floor() as isize).clamp(0, w as isize - 1) as usize;
    let y0 = (y.floor() as isize).clamp(0, h as isize - 1) as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = plane[y0 * w + x0];
    let p10 = plane[y0 * w + x1];
    let p01 = plane[y1 * w + x0];
    let p11 = plane[y1 * w + x1];

    let top = p00 + fx * (p10 - p00);
    let bot = p01 + fx * (p11 - p01);
    top + fy * (bot - top)
}

/// Apply radial shift to a chroma plane.
fn shift_plane_radial(src: &[f32], dst: &mut [f32], w: usize, h: usize, shift: f32) {
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let diag = (cx * cx + cy * cy).sqrt();
    if diag < 1.0 {
        dst[..w * h].copy_from_slice(&src[..w * h]);
        return;
    }

    for y in 0..h {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let r = (dx * dx + dy * dy).sqrt();

            // Radial scale: sample from a slightly different position
            // shift > 0 means the channel was displaced outward, so we
            // sample inward to correct
            let scale = 1.0 - shift * (r / diag);
            let sx = cx + dx * scale - 0.5;
            let sy = cy + dy * scale - 0.5;

            dst[y * w + x] = sample_bilinear(src, w, h, sx, sy);
        }
    }
}

impl Filter for ChromaticAberration {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        // Radial shift displaces pixels by up to |shift| * diagonal/2.
        // Max vertical displacement ≈ height/2 * |shift|.
        // Max horizontal displacement ≈ width/2 * |shift|.
        // Report the larger of the two.
        let max_shift = self.shift_a.abs().max(self.shift_b.abs());
        let max_disp = (width.max(height) as f32 / 2.0) * max_shift;
        (max_disp + 1.0).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;

        if self.shift_a.abs() > 1e-7 {
            let mut dst = ctx.take_f32(w * h);
            shift_plane_radial(&planes.a, &mut dst, w, h, self.shift_a);
            let old = core::mem::replace(&mut planes.a, dst);
            ctx.return_f32(old);
        }

        if self.shift_b.abs() > 1e-7 {
            let mut dst = ctx.take_f32(w * h);
            shift_plane_radial(&planes.b, &mut dst, w, h, self.shift_b);
            let old = core::mem::replace(&mut planes.b, dst);
            ctx.return_f32(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_shift_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.a {
            *v = 0.1;
        }
        let orig = planes.a.clone();
        ChromaticAberration::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, orig);
    }

    #[test]
    fn does_not_modify_luminance() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let l_orig = planes.l.clone();
        let mut ca = ChromaticAberration::default();
        ca.shift_a = 0.01;
        ca.shift_b = -0.005;
        ca.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }

    #[test]
    fn shift_changes_edge_pixels() {
        let mut planes = OklabPlanes::new(64, 64);
        // Create a radial chroma pattern
        for y in 0..64 {
            for x in 0..64 {
                let dx = x as f32 - 32.0;
                let dy = y as f32 - 32.0;
                let r = (dx * dx + dy * dy).sqrt() / 32.0;
                planes.a[y * 64 + x] = r * 0.1;
            }
        }
        let orig_corner = planes.a[0];

        let mut ca = ChromaticAberration::default();
        ca.shift_a = 0.02;
        ca.apply(&mut planes, &mut FilterContext::new());

        let new_corner = planes.a[0];
        assert!(
            (new_corner - orig_corner).abs() > 0.001,
            "corner should change: {orig_corner} -> {new_corner}"
        );
    }

    #[test]
    fn center_minimally_affected() {
        let mut planes = OklabPlanes::new(64, 64);
        // Uniform chroma
        for v in &mut planes.a {
            *v = 0.1;
        }
        let center_orig = planes.a[32 * 64 + 32];

        let mut ca = ChromaticAberration::default();
        ca.shift_a = 0.01;
        ca.apply(&mut planes, &mut FilterContext::new());

        let center_new = planes.a[32 * 64 + 32];
        assert!(
            (center_new - center_orig).abs() < 0.005,
            "center should barely change: {center_orig} -> {center_new}"
        );
    }
}
