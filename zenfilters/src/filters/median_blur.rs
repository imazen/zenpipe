use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Median filter for salt-and-pepper noise removal.
///
/// Replaces each pixel with the median of its neighborhood. Unlike Gaussian
/// blur, the median filter preserves edges while removing impulse noise.
/// Operates in Oklab space — L-only mode removes luminance noise without
/// affecting color; all-channel mode handles chroma noise too.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct MedianBlur {
    /// Neighborhood radius in pixels. Kernel size = 2*radius + 1.
    /// Practical range: 1–3. Radius 1 = 3×3 (9 samples), radius 2 = 5×5 (25).
    pub radius: u32,
    /// Whether to filter chroma (a, b) channels in addition to L.
    pub filter_chroma: bool,
}

impl Default for MedianBlur {
    fn default() -> Self {
        Self {
            radius: 1,
            filter_chroma: false,
        }
    }
}

impl Filter for MedianBlur {
    fn channel_access(&self) -> ChannelAccess {
        if self.filter_chroma {
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
        self.radius
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::MedianBlur
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
        let kernel_size = (2 * r + 1) * (2 * r + 1);

        // Scratch for the neighborhood window
        let mut window = ctx.take_f32(kernel_size);

        // Always filter L
        let mut dst = ctx.take_f32(w * h);
        median_filter_plane(&planes.l, &mut dst, w, h, r, &mut window);
        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);

        // Optionally filter chroma
        if self.filter_chroma {
            let mut dst_a = ctx.take_f32(w * h);
            median_filter_plane(&planes.a, &mut dst_a, w, h, r, &mut window);
            let old_a = core::mem::replace(&mut planes.a, dst_a);
            ctx.return_f32(old_a);

            let mut dst_b = ctx.take_f32(w * h);
            median_filter_plane(&planes.b, &mut dst_b, w, h, r, &mut window);
            let old_b = core::mem::replace(&mut planes.b, dst_b);
            ctx.return_f32(old_b);
        }

        ctx.return_f32(window);
    }
}

/// Apply median filter to a single f32 plane with edge replication.
fn median_filter_plane(
    src: &[f32],
    dst: &mut [f32],
    w: usize,
    h: usize,
    radius: usize,
    window: &mut [f32],
) {
    let r = radius;

    for y in 0..h {
        for x in 0..w {
            let mut count = 0;

            for ky in 0..2 * r + 1 {
                for kx in 0..2 * r + 1 {
                    // Edge replication: clamp to valid range
                    let sy = (y + ky).saturating_sub(r).min(h - 1);
                    let sx = (x + kx).saturating_sub(r).min(w - 1);
                    window[count] = src[sy * w + sx];
                    count += 1;
                }
            }

            // Find median using partial sort (O(n) average)
            let mid = count / 2;
            window[..count].select_nth_unstable_by(mid, |a, b| {
                a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal)
            });
            dst[y * w + x] = window[mid];
        }
    }
}

static MEDIAN_BLUR_SCHEMA: FilterSchema = FilterSchema {
    name: "median_blur",
    label: "Median Blur",
    description: "Median filter for impulse noise removal (preserves edges)",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "radius",
            label: "Radius",
            description: "Neighborhood radius (1 = 3×3, 2 = 5×5, 3 = 7×7)",
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
            name: "filter_chroma",
            label: "Filter Chroma",
            description: "Also apply median to color channels (a, b)",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for MedianBlur {
    fn schema() -> &'static FilterSchema {
        &MEDIAN_BLUR_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "radius" => Some(ParamValue::Int(self.radius as i32)),
            "filter_chroma" => Some(ParamValue::Bool(self.filter_chroma)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "radius" => {
                if let Some(v) = value.as_i32() {
                    self.radius = (v as u32).clamp(1, 5);
                    true
                } else {
                    false
                }
            }
            "filter_chroma" => {
                if let ParamValue::Bool(v) = value {
                    self.filter_chroma = v;
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
    fn zero_radius_is_identity() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        MedianBlur {
            radius: 0,
            filter_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn constant_plane_stays_constant() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.42);
        MedianBlur {
            radius: 2,
            filter_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!(
                (v - 0.42).abs() < 1e-6,
                "constant plane should stay constant, got {v}"
            );
        }
    }

    #[test]
    fn removes_salt_and_pepper() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        // Add a single "salt" pixel
        let salt_idx = planes.index(16, 16);
        planes.l[salt_idx] = 1.0;
        // Add a single "pepper" pixel
        let pepper_idx = planes.index(10, 10);
        planes.l[pepper_idx] = 0.0;

        MedianBlur {
            radius: 1,
            filter_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Salt and pepper should be replaced by median of neighbors (0.5)
        let salt = planes.l[salt_idx];
        let pepper = planes.l[pepper_idx];
        assert!(
            (salt - 0.5).abs() < 1e-6,
            "salt pixel should be median-filtered to 0.5, got {salt}"
        );
        assert!(
            (pepper - 0.5).abs() < 1e-6,
            "pepper pixel should be median-filtered to 0.5, got {pepper}"
        );
    }

    #[test]
    fn preserves_edges() {
        let mut planes = OklabPlanes::new(32, 32);
        // Sharp step edge at x=16
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        MedianBlur {
            radius: 1,
            filter_chroma: false,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Interior pixels should be unchanged
        let left = planes.l[planes.index(8, 16)];
        let right = planes.l[planes.index(24, 16)];
        assert!(
            (left - 0.2).abs() < 1e-6,
            "interior left should be 0.2, got {left}"
        );
        assert!(
            (right - 0.8).abs() < 1e-6,
            "interior right should be 0.8, got {right}"
        );
    }

    #[test]
    fn filters_chroma_when_enabled() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.a.fill(0.1);
        // Add chroma noise
        let noise_idx = planes.index(8, 8);
        planes.a[noise_idx] = 0.5;

        MedianBlur {
            radius: 1,
            filter_chroma: true,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let noise_pixel = planes.a[noise_idx];
        assert!(
            (noise_pixel - 0.1).abs() < 1e-6,
            "chroma noise should be filtered, got {noise_pixel}"
        );
    }
}
