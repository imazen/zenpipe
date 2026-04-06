use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Directional (motion) blur along an arbitrary angle.
///
/// Simulates motion blur by averaging pixels along a line at the given angle.
/// The `length` parameter controls the blur distance in pixels, and `angle`
/// sets the direction in degrees (0 = horizontal right, 90 = vertical down).
///
/// Works by sampling `length` points along the direction vector and averaging.
/// Edge pixels are clamped (replicated).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct MotionBlur {
    /// Blur angle in degrees. 0 = horizontal right, 90 = vertical down.
    pub angle: f32,
    /// Blur length in pixels. Must be >= 1.
    pub length: f32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        Self {
            angle: 0.0,
            length: 10.0,
        }
    }
}

impl Filter for MotionBlur {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.length / 2.0).ceil() as u32
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PostResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.length < 1.5 {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;
        let a = self.angle.to_radians();
        let dx = a.cos();
        let dy = a.sin();
        let half_len = self.length / 2.0;
        let steps = self.length.ceil() as usize;
        let inv_steps = 1.0 / steps as f32;

        let blur_plane = |src: &mut alloc::vec::Vec<f32>, ctx: &mut FilterContext| {
            let n = w * h;
            let mut dst = ctx.take_f32(n);

            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for s in 0..steps {
                        let t = (s as f32 / (steps - 1).max(1) as f32) * 2.0 - 1.0;
                        let sx = (x as f32 + t * half_len * dx).round();
                        let sy = (y as f32 + t * half_len * dy).round();
                        let sx = sx.clamp(0.0, (w - 1) as f32) as usize;
                        let sy = sy.clamp(0.0, (h - 1) as f32) as usize;
                        sum += src[sy * w + sx];
                    }
                    dst[y * w + x] = (sum * inv_steps).clamp(0.0, 1.0);
                }
            }

            let old = core::mem::replace(src, dst);
            ctx.return_f32(old);
        };

        blur_plane(&mut planes.l, ctx);
        blur_plane(&mut planes.a, ctx);
        blur_plane(&mut planes.b, ctx);
        if let Some(alpha) = &mut planes.alpha {
            blur_plane(alpha, ctx);
        }
    }
}

// ─── Zoom blur ─────────────────────────────────────────────────────

/// Radial zoom blur — blurs pixels along lines radiating from a center point.
///
/// Simulates the effect of zooming a camera lens during exposure.
/// Pixels near the center are sharp; pixels farther from center are
/// increasingly blurred along radial lines.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ZoomBlur {
    /// Blur strength (0 = no effect, 1 = full zoom blur).
    pub amount: f32,
    /// Center X as fraction of image width (0.5 = center).
    pub center_x: f32,
    /// Center Y as fraction of image height (0.5 = center).
    pub center_y: f32,
}

impl Default for ZoomBlur {
    fn default() -> Self {
        Self {
            amount: 0.3,
            center_x: 0.5,
            center_y: 0.5,
        }
    }
}

impl Filter for ZoomBlur {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        let max_dim = width.max(height) as f32;
        (self.amount * max_dim * 0.5).ceil() as u32
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PostResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount < 0.001 {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;
        let cx = self.center_x * w as f32;
        let cy = self.center_y * h as f32;
        let max_dist = ((w * w + h * h) as f32).sqrt() * 0.5;
        let samples = 16usize;
        let inv_samples = 1.0 / samples as f32;

        let blur_plane = |src: &mut alloc::vec::Vec<f32>, ctx: &mut FilterContext| {
            let n = w * h;
            let mut dst = ctx.take_f32(n);

            for y in 0..h {
                for x in 0..w {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let blur_len = self.amount * dist / max_dist;

                    if blur_len < 0.5 {
                        dst[y * w + x] = src[y * w + x];
                        continue;
                    }

                    let mut sum = 0.0f32;
                    for s in 0..samples {
                        let t = (s as f32 / (samples - 1) as f32) * 2.0 - 1.0;
                        let sx = (x as f32 + t * blur_len * dx / dist.max(1e-6)).round();
                        let sy = (y as f32 + t * blur_len * dy / dist.max(1e-6)).round();
                        let sx = sx.clamp(0.0, (w - 1) as f32) as usize;
                        let sy = sy.clamp(0.0, (h - 1) as f32) as usize;
                        sum += src[sy * w + sx];
                    }
                    dst[y * w + x] = (sum * inv_samples).clamp(0.0, 1.0);
                }
            }

            let old = core::mem::replace(src, dst);
            ctx.return_f32(old);
        };

        blur_plane(&mut planes.l, ctx);
        blur_plane(&mut planes.a, ctx);
        blur_plane(&mut planes.b, ctx);
        if let Some(alpha) = &mut planes.alpha {
            blur_plane(alpha, ctx);
        }
    }
}

// ─── Param schemas ─────────────────────────────────────────────────

static MOTION_BLUR_SCHEMA: FilterSchema = FilterSchema {
    name: "motion_blur",
    label: "Motion Blur",
    description: "Directional blur along an arbitrary angle",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "angle",
            label: "Angle",
            description: "Blur direction in degrees (0 = horizontal right, 90 = vertical down)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 360.0,
                default: 0.0,
                identity: 0.0,
                step: 5.0,
            },
            unit: "°",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "length",
            label: "Length",
            description: "Blur distance in pixels",
            kind: ParamKind::Float {
                min: 0.0,
                max: 200.0,
                default: 10.0,
                identity: 0.0,
                step: 1.0,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for MotionBlur {
    fn schema() -> &'static FilterSchema {
        &MOTION_BLUR_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "angle" => Some(ParamValue::Float(self.angle)),
            "length" => Some(ParamValue::Float(self.length)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "angle" => {
                if let Some(v) = value.as_f32() {
                    self.angle = v;
                    true
                } else {
                    false
                }
            }
            "length" => {
                if let Some(v) = value.as_f32() {
                    self.length = v.max(0.0);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

static ZOOM_BLUR_SCHEMA: FilterSchema = FilterSchema {
    name: "zoom_blur",
    label: "Zoom Blur",
    description: "Radial zoom blur from center point",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Blur strength (0 = none, 1 = full)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.3,
                identity: 0.0,
                step: 0.05,
            },
            unit: "×",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "center_x",
            label: "Center X",
            description: "Center X as fraction of width",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "center_y",
            label: "Center Y",
            description: "Center Y as fraction of height",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for ZoomBlur {
    fn schema() -> &'static FilterSchema {
        &ZOOM_BLUR_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "amount" => Some(ParamValue::Float(self.amount)),
            "center_x" => Some(ParamValue::Float(self.center_x)),
            "center_y" => Some(ParamValue::Float(self.center_y)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "amount" => {
                if let Some(v) = value.as_f32() {
                    self.amount = v;
                    true
                } else {
                    false
                }
            }
            "center_x" => {
                if let Some(v) = value.as_f32() {
                    self.center_x = v;
                    true
                } else {
                    false
                }
            }
            "center_y" => {
                if let Some(v) = value.as_f32() {
                    self.center_y = v;
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
    fn short_length_is_identity() {
        let mb = MotionBlur {
            angle: 0.0,
            length: 1.0,
        };
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 256.0).clamp(0.0, 1.0);
        }
        let orig = planes.l.clone();
        mb.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig, "length=1 should be identity");
    }

    #[test]
    fn horizontal_motion_blur_smooths_columns() {
        let mb = MotionBlur {
            angle: 0.0,
            length: 8.0,
        };
        let mut planes = OklabPlanes::new(32, 32);
        // Vertical stripe pattern
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        mb.apply(&mut planes, &mut FilterContext::new());
        // Edge should be blurred horizontally
        let edge = planes.l[planes.index(16, 16)];
        assert!(
            edge > 0.3 && edge < 0.7,
            "horizontal blur should smooth edge, got {edge}"
        );
    }

    #[test]
    fn zoom_blur_identity_at_center() {
        let zb = ZoomBlur {
            amount: 0.5,
            center_x: 0.5,
            center_y: 0.5,
        };
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        let idx = planes.index(16, 16);
        planes.l[idx] = 0.8;
        let center_val = planes.l[planes.index(16, 16)];
        zb.apply(&mut planes, &mut FilterContext::new());
        // Center pixel should be unchanged (zero distance)
        let after = planes.l[planes.index(16, 16)];
        assert!(
            (after - center_val).abs() < 0.05,
            "center should be near-unchanged: before={center_val}, after={after}"
        );
    }
}
