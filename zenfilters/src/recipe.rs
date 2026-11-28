//! Typed, validated filter recipes and prepared descriptor-aware filter execution.
//!
//! [`FilterRecipe`] is an owned, serializable (with `serde`), `no_std + alloc`
//! description of a filter chain that carries no pixel buffers or runtime handles.
//! [`FilterRecipe::prepare`] binds it to an exact active `(Primaries, DiffuseWhite)`
//! domain and image dimensions, computes exact spatial halo and scratch byte quotes
//! ([`FilterSupport`], [`FilterScratchQuote`]), and produces an immutable
//! [`PreparedFilter`] supporting both packed and native strided row execution with
//! cooperative [`how_far::Pulse`] cancellation.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use whereat::{At, at};
use zenpixels::{DiffuseWhite, Primaries};

use crate::context::FilterContext;
use crate::filter::Filter;
use crate::filters;
use crate::gamut_map::GamutMapping;
use crate::pipeline::{Pipeline, PipelineConfig, PipelineError, WorkingSpace};

/// Validation error when constructing a [`FilterOp`] or [`FilterRecipe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterRecipeError {
    /// A floating-point parameter was NaN or infinite.
    NonFiniteParameter,
    /// A parameter fell outside its defined mathematical domain.
    OutOfRange(&'static str),
    /// Reference width was zero.
    ZeroReferenceWidth,
}

impl fmt::Display for FilterRecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteParameter => write!(f, "filter parameter must be finite"),
            Self::OutOfRange(field) => write!(f, "filter parameter `{field}` out of range"),
            Self::ZeroReferenceWidth => write!(f, "reference width must be non-zero"),
        }
    }
}

impl core::error::Error for FilterRecipeError {}

/// Spatial support required by a [`PreparedFilter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterSupport {
    /// Pure per-pixel transform (zero halo, supports arbitrary horizontal strip streaming).
    Point,
    /// Neighborhood spatial filter requiring `max_radius` per filter and `total_halo`
    /// composed vertical/horizontal padding across chained spatial passes.
    Window {
        /// Maximum single-filter neighborhood radius in pixels.
        max_radius: u32,
        /// Composed total halo (sum of radii across chained spatial filters) in pixels.
        total_halo: u32,
    },
}

/// Conservative memory quote for executing a [`PreparedFilter`] on a given image extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterScratchQuote {
    /// Bytes required for planar Oklab/RGB f32 working planes (3 or 4 planes).
    pub planar_bytes: u64,
    /// Maximum rows processed per strip (`height` when full-frame is required).
    pub strip_rows: u32,
    /// Whether neighborhood support requires materializing full-frame planar scratch.
    pub full_frame_required: bool,
}

/// One validated, portable photo-filter operation.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum FilterOp {
    /// Exposure shift in photographic stops (`[-10.0, 10.0]`).
    Exposure { stops: f32 },
    /// Perceptual contrast adjustment (`[-1.0, 2.0]`).
    Contrast { amount: f32 },
    /// Oklab chroma saturation factor (`[0.0, 4.0]`; `1.0` is identity).
    Saturation { factor: f32 },
    /// Selective chroma boost protecting skin tones (`amount` in `[-1.0, 2.0]`,
    /// `protection` in `[0.0, 1.0]`).
    Vibrance { amount: f32, protection: f32 },
    /// Color temperature shift (`[-1.0, 1.0]`).
    Temperature { shift: f32 },
    /// Green-magenta tint shift (`[-1.0, 1.0]`).
    Tint { shift: f32 },
    /// Highlight and shadow recovery (`highlights`, `shadows` in `[-1.0, 1.0]`).
    HighlightsShadows { highlights: f32, shadows: f32 },
    /// Unsharp mask sharpen (`sigma` in `(0.0, 64.0]`, `amount` in `[0.0, 5.0]`).
    Sharpen { sigma: f32, amount: f32 },
    /// Separable Gaussian blur (`sigma` in `(0.0, 128.0]`).
    Blur { sigma: f32 },
    /// Local midtone contrast (`sigma` in `(0.0, 64.0]`, `amount` in `[-1.0, 2.0]`).
    Clarity { sigma: f32, amount: f32 },
    /// Perceptual luminance grayscale conversion.
    Grayscale,
    /// Value inversion.
    Invert,
    /// Sepia toning (`amount` in `[0.0, 1.0]`).
    Sepia { amount: f32 },
}

fn check_range(v: f32, lo: f32, hi: f32, name: &'static str) -> Result<f32, FilterRecipeError> {
    if !v.is_finite() {
        return Err(FilterRecipeError::NonFiniteParameter);
    }
    if v < lo || v > hi {
        return Err(FilterRecipeError::OutOfRange(name));
    }
    Ok(v)
}

impl FilterOp {
    /// Validated exposure op in stops (`[-10.0, 10.0]`).
    pub fn exposure(stops: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Exposure {
            stops: check_range(stops, -10.0, 10.0, "stops")?,
        })
    }

    /// Validated contrast op (`[-1.0, 2.0]`).
    pub fn contrast(amount: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Contrast {
            amount: check_range(amount, -1.0, 2.0, "amount")?,
        })
    }

    /// Validated saturation factor (`[0.0, 4.0]`).
    pub fn saturation(factor: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Saturation {
            factor: check_range(factor, 0.0, 4.0, "factor")?,
        })
    }

    /// Validated vibrance op.
    pub fn vibrance(amount: f32, protection: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Vibrance {
            amount: check_range(amount, -1.0, 2.0, "amount")?,
            protection: check_range(protection, 0.0, 1.0, "protection")?,
        })
    }

    /// Validated temperature op (`[-1.0, 1.0]`).
    pub fn temperature(shift: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Temperature {
            shift: check_range(shift, -1.0, 1.0, "shift")?,
        })
    }

    /// Validated tint op (`[-1.0, 1.0]`).
    pub fn tint(shift: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Tint {
            shift: check_range(shift, -1.0, 1.0, "shift")?,
        })
    }

    /// Validated highlights/shadows op (`[-1.0, 1.0]`).
    pub fn highlights_shadows(highlights: f32, shadows: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::HighlightsShadows {
            highlights: check_range(highlights, -1.0, 1.0, "highlights")?,
            shadows: check_range(shadows, -1.0, 1.0, "shadows")?,
        })
    }

    /// Validated unsharp-mask sharpen op.
    pub fn sharpen(sigma: f32, amount: f32) -> Result<Self, FilterRecipeError> {
        let sigma = check_range(sigma, f32::MIN_POSITIVE, 64.0, "sigma")?;
        let amount = check_range(amount, 0.0, 5.0, "amount")?;
        Ok(Self::Sharpen { sigma, amount })
    }

    /// Validated Gaussian blur op.
    pub fn blur(sigma: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Blur {
            sigma: check_range(sigma, f32::MIN_POSITIVE, 128.0, "sigma")?,
        })
    }

    /// Validated clarity op.
    pub fn clarity(sigma: f32, amount: f32) -> Result<Self, FilterRecipeError> {
        let sigma = check_range(sigma, f32::MIN_POSITIVE, 64.0, "sigma")?;
        let amount = check_range(amount, -1.0, 2.0, "amount")?;
        Ok(Self::Clarity { sigma, amount })
    }

    /// Validated sepia op (`[0.0, 1.0]`).
    pub fn sepia(amount: f32) -> Result<Self, FilterRecipeError> {
        Ok(Self::Sepia {
            amount: check_range(amount, 0.0, 1.0, "amount")?,
        })
    }

    /// Re-validate every parameter (for deserialized instances).
    pub fn validate(&self) -> Result<(), FilterRecipeError> {
        match *self {
            Self::Exposure { stops } => {
                check_range(stops, -10.0, 10.0, "stops")?;
            }
            Self::Contrast { amount } => {
                check_range(amount, -1.0, 2.0, "amount")?;
            }
            Self::Saturation { factor } => {
                check_range(factor, 0.0, 4.0, "factor")?;
            }
            Self::Vibrance { amount, protection } => {
                check_range(amount, -1.0, 2.0, "amount")?;
                check_range(protection, 0.0, 1.0, "protection")?;
            }
            Self::Temperature { shift } => {
                check_range(shift, -1.0, 1.0, "shift")?;
            }
            Self::Tint { shift } => {
                check_range(shift, -1.0, 1.0, "shift")?;
            }
            Self::HighlightsShadows {
                highlights,
                shadows,
            } => {
                check_range(highlights, -1.0, 1.0, "highlights")?;
                check_range(shadows, -1.0, 1.0, "shadows")?;
            }
            Self::Sharpen { sigma, amount } => {
                check_range(sigma, f32::MIN_POSITIVE, 64.0, "sigma")?;
                check_range(amount, 0.0, 5.0, "amount")?;
            }
            Self::Blur { sigma } => {
                check_range(sigma, f32::MIN_POSITIVE, 128.0, "sigma")?;
            }
            Self::Clarity { sigma, amount } => {
                check_range(sigma, f32::MIN_POSITIVE, 64.0, "sigma")?;
                check_range(amount, -1.0, 2.0, "amount")?;
            }
            Self::Grayscale | Self::Invert => {}
            Self::Sepia { amount } => {
                check_range(amount, 0.0, 1.0, "amount")?;
            }
        }
        Ok(())
    }

    fn build_filter(&self) -> Box<dyn Filter> {
        match *self {
            Self::Exposure { stops } => Box::new(filters::Exposure { stops }),
            Self::Contrast { amount } => Box::new(filters::Contrast { amount }),
            Self::Saturation { factor } => Box::new(filters::Saturation { factor }),
            Self::Vibrance { amount, protection } => {
                Box::new(filters::Vibrance { amount, protection })
            }
            Self::Temperature { shift } => Box::new(filters::Temperature { shift }),
            Self::Tint { shift } => Box::new(filters::Tint { shift }),
            Self::HighlightsShadows {
                highlights,
                shadows,
            } => {
                let mut hs = filters::HighlightsShadows::default();
                hs.highlights = highlights;
                hs.shadows = shadows;
                Box::new(hs)
            }
            Self::Sharpen { sigma, amount } => Box::new(filters::Sharpen { sigma, amount }),
            Self::Blur { sigma } => Box::new(filters::Blur { sigma }),
            Self::Clarity { sigma, amount } => {
                let mut c = filters::Clarity::default();
                c.sigma = sigma;
                c.amount = amount;
                Box::new(c)
            }
            Self::Grayscale => Box::new(filters::Grayscale),
            Self::Invert => Box::new(filters::Invert),
            Self::Sepia { amount } => Box::new(filters::Sepia { amount }),
        }
    }
}

/// Portable, descriptor-agnostic filter chain recipe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterRecipe {
    ops: Vec<FilterOp>,
    working_space: WorkingSpace,
    gamut_mapping: GamutMapping,
    reference_width: Option<u32>,
}

impl FilterRecipe {
    /// Create an empty Oklab filter recipe.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a validated [`FilterOp`].
    pub fn with_op(mut self, op: FilterOp) -> Result<Self, FilterRecipeError> {
        op.validate()?;
        self.ops.push(op);
        Ok(self)
    }

    /// Push a validated [`FilterOp`] in place.
    pub fn push(&mut self, op: FilterOp) -> Result<(), FilterRecipeError> {
        op.validate()?;
        self.ops.push(op);
        Ok(())
    }

    /// Select the working color space for filter evaluation.
    #[must_use]
    pub fn with_working_space(mut self, space: WorkingSpace) -> Self {
        self.working_space = space;
        self
    }

    /// Select post-filter gamut mapping.
    #[must_use]
    pub fn with_gamut_mapping(mut self, mapping: GamutMapping) -> Self {
        self.gamut_mapping = mapping;
        self
    }

    /// Set a resolution-independent reference width for pixel-space sigmas.
    pub fn with_reference_width(mut self, width: u32) -> Result<Self, FilterRecipeError> {
        if width == 0 {
            return Err(FilterRecipeError::ZeroReferenceWidth);
        }
        self.reference_width = Some(width);
        Ok(self)
    }

    /// Borrow the ordered filter operations.
    #[must_use]
    pub fn ops(&self) -> &[FilterOp] {
        &self.ops
    }

    /// Prepare this recipe for an exact color domain (`primaries`, `diffuse_white`)
    /// and image extent `(width, height, has_alpha)`.
    ///
    /// Unlike legacy convenience paths, this never guesses BT.709 when `primaries` is
    /// [`Primaries::Unknown`] and preserves wide-gamut primaries (Display P3, BT.2020)
    /// and explicit [`DiffuseWhite`] anchors through the compiled [`Pipeline`].
    #[track_caller]
    pub fn prepare(
        &self,
        primaries: Primaries,
        diffuse_white: DiffuseWhite,
        width: u32,
        height: u32,
        has_alpha: bool,
    ) -> Result<PreparedFilter, At<PipelineError>> {
        // Normalize linear reference white relative to SDR 1.0 or explicit nits anchor.
        // PipelineConfig::reference_white expects 1.0 for relative-linear SDR and
        // `diffuse_white.nits()` when operating on absolute-nit linear data; carry the
        // explicit anchor normalized to BT.2408 (203 nits => 1.0 relative) unless absolute.
        let reference_white = (diffuse_white.nits() / DiffuseWhite::BT2408.nits()).max(f32::MIN_POSITIVE);
        let cfg = PipelineConfig::new(self.working_space)
            .with_primaries(primaries)
            .with_reference_white(reference_white)
            .with_gamut_mapping(self.gamut_mapping)
            .with_reference_width(self.reference_width);
        let mut pipeline = Pipeline::new(cfg)?;
        for op in &self.ops {
            let filter = op.build_filter();
            // Validate working-space compatibility before pushing (avoiding panic on mismatch).
            let semantics = filter.plane_semantics();
            let compat = match semantics {
                crate::filter::PlaneSemantics::Any => true,
                crate::filter::PlaneSemantics::Oklab => self.working_space == WorkingSpace::Oklab,
                crate::filter::PlaneSemantics::Rgb => {
                    matches!(
                        self.working_space,
                        WorkingSpace::Srgb | WorkingSpace::LinearRgb
                    )
                }
            };
            if !compat {
                return Err(at!(PipelineError::UnsupportedPrimaries(primaries)));
            }
            pipeline.push(filter);
        }
        pipeline.scale_to_width(width);

        let full_frame_required = pipeline.has_neighborhood_filter();
        let support = if full_frame_required {
            FilterSupport::Window {
                max_radius: pipeline.max_neighborhood_radius(width, height),
                total_halo: pipeline.total_halo(width, height),
            }
        } else {
            FilterSupport::Point
        };
        let strip_rows = if full_frame_required {
            height
        } else {
            (crate::pipeline::strip_height(width, has_alpha, 0) as u32).min(height.max(1))
        };
        let planes = if has_alpha { 4u64 } else { 3u64 };
        let planar_bytes = u64::from(width)
            .saturating_mul(u64::from(strip_rows))
            .saturating_mul(planes)
            .saturating_mul(core::mem::size_of::<f32>() as u64);
        // Account for per-operation FilterContext scratch buffers using an upper bound across
        // ALL blur implementations (direct FIR convolution, scalar stackblur with two transpose
        // planes + circular stack, and AVX2/SIMD stackblur with f32x8 vertical stack).
        let op_scratch_bytes = if full_frame_required {
            let max_r = match support {
                FilterSupport::Window { max_radius, .. } => u64::from(max_radius),
                FilterSupport::Point => 0,
            };
            let single_plane = u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(core::mem::size_of::<f32>() as u64);
            let max_dim = u64::from(width.max(height));
            let fir_scratch = single_plane.saturating_mul(2).saturating_add(
                max_dim
                    .saturating_add(max_r.saturating_mul(2))
                    .saturating_mul(core::mem::size_of::<f32>() as u64),
            );
            let stack_elems = max_r.saturating_mul(2).saturating_add(1);
            let stack_scalar = single_plane.saturating_mul(4).saturating_add(
                stack_elems.saturating_mul(core::mem::size_of::<f32>() as u64),
            );
            let stack_simd = single_plane
                .saturating_mul(2)
                .saturating_add(
                    // f32x8 vertical stack (8) + scalar tail stack (1) live simultaneously!
                    stack_elems.saturating_mul(9 * core::mem::size_of::<f32>() as u64),
                )
                .saturating_add(
                    // scalar tail col_buf + col_out (2 * height f32s)
                    u64::from(height).saturating_mul(2 * core::mem::size_of::<f32>() as u64),
                );
            fir_scratch.max(stack_scalar).max(stack_simd)
        } else {
            0
        };
        Ok(PreparedFilter {
            pipeline,
            primaries,
            diffuse_white,
            width,
            height,
            has_alpha,
            support,
            scratch: FilterScratchQuote {
                planar_bytes,
                strip_rows,
                full_frame_required,
            },
            op_scratch_bytes,
        })
    }
}

/// Prepared, descriptor-bound filter execution unit with exact halo and scratch quotes.
pub struct PreparedFilter {
    pipeline: Pipeline,
    primaries: Primaries,
    diffuse_white: DiffuseWhite,
    width: u32,
    height: u32,
    has_alpha: bool,
    support: FilterSupport,
    scratch: FilterScratchQuote,
    op_scratch_bytes: u64,
}

impl PreparedFilter {
    /// Active working primaries bound at preparation.
    #[must_use]
    pub const fn primaries(&self) -> Primaries {
        self.primaries
    }

    /// Active diffuse white anchor bound at preparation.
    #[must_use]
    pub const fn diffuse_white(&self) -> DiffuseWhite {
        self.diffuse_white
    }

    /// Bound image dimensions `(width, height)`.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Total conservative workspace bytes (planar buffers + per-operation scratch buffers).
    #[must_use]
    pub const fn total_workspace_bytes(&self) -> u64 {
        self.scratch.planar_bytes.saturating_add(self.op_scratch_bytes)
    }

    /// Spatial support required by this filter chain.
    #[must_use]
    pub const fn support(&self) -> FilterSupport {
        self.support
    }

    /// Conservative planar scratch memory quote.
    #[must_use]
    pub const fn scratch_quote(&self) -> FilterScratchQuote {
        self.scratch
    }

    /// Apply to tightly packed linear f32 RGB/RGBA slices with cooperative cancellation.
    ///
    /// Samples use straight (unpremultiplied) alpha when `has_alpha` is true, and relative
    /// linear `1.0` represents the bound [`diffuse_white`](Self::diffuse_white) anchor
    /// (e.g. 203 cd/m² for BT.2408).
    #[track_caller]
    pub fn apply_with_pulse(
        &self,
        src: &[f32],
        dst: &mut [f32],
        ctx: &mut FilterContext,
        pulse: Option<&dyn how_far::Pulse>,
    ) -> Result<(), At<PipelineError>> {
        let channels = if self.has_alpha { 4 } else { 3 };
        self.pipeline
            .apply_with_pulse(src, dst, self.width, self.height, channels, ctx, pulse)
    }

    /// Apply natively to strided linear f32 rows (`src_stride_elems` / `dst_stride_elems`
    /// in `f32` elements, `>= width * channels`) at zero copy cost when tightly packed and
    /// zero full-frame interleaved repack allocation when strided.
    ///
    /// Samples use straight (unpremultiplied) alpha when `has_alpha` is true, and relative
    /// linear `1.0` represents the bound [`diffuse_white`](Self::diffuse_white) anchor.
    #[track_caller]
    pub fn apply_strided_with_pulse(
        &self,
        src: &[f32],
        src_stride_elems: usize,
        dst: &mut [f32],
        dst_stride_elems: usize,
        ctx: &mut FilterContext,
        pulse: Option<&dyn how_far::Pulse>,
    ) -> Result<(), At<PipelineError>> {
        let channels = if self.has_alpha { 4usize } else { 3usize };
        let row_elems = (self.width as usize)
            .checked_mul(channels)
            .ok_or_else(|| {
                at!(PipelineError::BufferSize {
                    expected: usize::MAX,
                    actual: src.len(),
                })
            })?;
        let h = self.height as usize;
        if src_stride_elems < row_elems || dst_stride_elems < row_elems {
            return Err(at!(PipelineError::BufferSize {
                expected: row_elems,
                actual: src_stride_elems.min(dst_stride_elems),
            }));
        }
        if h == 0 {
            return Ok(());
        }
        let needed_src = (h - 1)
            .checked_mul(src_stride_elems)
            .and_then(|o| o.checked_add(row_elems))
            .ok_or_else(|| {
                at!(PipelineError::BufferSize {
                    expected: usize::MAX,
                    actual: src.len(),
                })
            })?;
        let needed_dst = (h - 1)
            .checked_mul(dst_stride_elems)
            .and_then(|o| o.checked_add(row_elems))
            .ok_or_else(|| {
                at!(PipelineError::BufferSize {
                    expected: usize::MAX,
                    actual: dst.len(),
                })
            })?;
        if src.len() < needed_src {
            return Err(at!(PipelineError::BufferSize {
                expected: needed_src,
                actual: src.len(),
            }));
        }
        if dst.len() < needed_dst {
            return Err(at!(PipelineError::BufferSize {
                expected: needed_dst,
                actual: dst.len(),
            }));
        }
        // Contiguous fast path wraps the exact packed kernel at zero extra copy cost.
        if src_stride_elems == row_elems && dst_stride_elems == row_elems {
            return self.apply_with_pulse(
                &src[..h * row_elems],
                &mut dst[..h * row_elems],
                ctx,
                pulse,
            );
        }
        // Strided row path:
        // - Point filters run row-by-row with zero full-frame copy.
        // - Neighborhood filters scatter/gather directly between strided rows and
        //   reusable FilterContext planar buffers (zero full-frame interleaved repack!).
        let stop = pulse.unwrap_or(&how_far::NoPulse);
        if !self.scratch.full_frame_required {
            for y in 0..h {
                stop.check().map_err(|r| at!(PipelineError::Cancelled(r)))?;
                let s0 = y * src_stride_elems;
                let d0 = y * dst_stride_elems;
                self.pipeline.apply_with_pulse(
                    &src[s0..s0 + row_elems],
                    &mut dst[d0..d0 + row_elems],
                    self.width,
                    1,
                    channels as u32,
                    ctx,
                    pulse,
                )?;
            }
            return Ok(());
        }
        self.pipeline.apply_full_frame_strided(
            src,
            src_stride_elems,
            dst,
            dst_stride_elems,
            self.width,
            self.height,
            channels as u32,
            ctx,
            stop,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_honors_p3_and_bt2020_and_refuses_unknown() {
        let recipe = FilterRecipe::new()
            .with_op(FilterOp::exposure(0.5).unwrap())
            .unwrap()
            .with_op(FilterOp::saturation(1.2).unwrap())
            .unwrap();
        assert!(
            recipe
                .prepare(Primaries::Unknown, DiffuseWhite::BT2408, 16, 16, false)
                .is_err()
        );
        let p3 = recipe
            .prepare(Primaries::DisplayP3, DiffuseWhite::BT2408, 16, 16, false)
            .unwrap();
        let bt709 = recipe
            .prepare(Primaries::Bt709, DiffuseWhite::BT2408, 16, 16, false)
            .unwrap();
        assert_eq!(p3.support(), FilterSupport::Point);
        assert_eq!(p3.primaries(), Primaries::DisplayP3);
        // Saturated red input must produce distinct linear values under Display P3 vs BT.709 matrices.
        let mut src = Vec::new();
        for _ in 0..(16 * 16) {
            src.extend_from_slice(&[0.8f32, 0.05, 0.02]);
        }
        let mut dst_p3 = alloc::vec![0.0f32; src.len()];
        let mut dst_709 = alloc::vec![0.0f32; src.len()];
        let mut ctx = FilterContext::new();
        p3.apply_with_pulse(&src, &mut dst_p3, &mut ctx, None)
            .unwrap();
        bt709
            .apply_with_pulse(&src, &mut dst_709, &mut ctx, None)
            .unwrap();
        assert_ne!(dst_p3, dst_709, "Display P3 and BT.709 primaries must not collapse");
    }

    #[test]
    fn recipe_strided_matches_packed_exact_and_quotes_composed_halo() {
        let recipe = FilterRecipe::new()
            .with_op(FilterOp::exposure(0.25).unwrap())
            .unwrap()
            .with_op(FilterOp::blur(1.5).unwrap())
            .unwrap()
            .with_op(FilterOp::sharpen(1.0, 0.4).unwrap())
            .unwrap();
        let prep = recipe
            .prepare(Primaries::Bt2020, DiffuseWhite::BT2408, 12, 10, true)
            .unwrap();
        let FilterSupport::Window {
            max_radius,
            total_halo,
        } = prep.support()
        else {
            panic!("expected Window support for blur + sharpen");
        };
        assert!(total_halo > max_radius && prep.scratch_quote().full_frame_required);
        let row_elems = 12 * 4;
        let stride = row_elems + 7; // intentionally odd padding
        let mut packed_src = alloc::vec![0.0f32; 10 * row_elems];
        let mut strided_src = alloc::vec![-99.0f32; 10 * stride];
        for y in 0..10 {
            for x in 0..row_elems {
                let v = ((y * 13 + x * 7) % 100) as f32 / 100.0;
                packed_src[y * row_elems + x] = v;
                strided_src[y * stride + x] = v;
            }
        }
        let mut packed_dst = alloc::vec![0.0f32; packed_src.len()];
        let mut strided_dst = alloc::vec![-77.0f32; 10 * stride];
        let mut ctx = FilterContext::new();
        prep.apply_with_pulse(&packed_src, &mut packed_dst, &mut ctx, None)
            .unwrap();
        prep.apply_strided_with_pulse(
            &strided_src,
            stride,
            &mut strided_dst,
            stride,
            &mut ctx,
            None,
        )
        .unwrap();
        for y in 0..10 {
            assert_eq!(
                &strided_dst[y * stride..y * stride + row_elems],
                &packed_dst[y * row_elems..(y + 1) * row_elems],
                "strided row {y} must match packed bit-exact"
            );
            // Padding tail must remain untouched.
            assert!(strided_dst[y * stride + row_elems..(y + 1) * stride].iter().all(|&v| v == -77.0));
        }
    }
}
