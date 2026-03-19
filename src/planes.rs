use crate::context::FilterContext;
use crate::prelude::*;

/// Owned planar f32 data for Oklab L, a, b, and optional alpha.
///
/// Each plane is a contiguous `Vec<f32>` of `width * height` elements,
/// stored in row-major order. This is the working representation that
/// filters operate on.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct OklabPlanes {
    pub width: u32,
    pub height: u32,
    pub l: Vec<f32>,
    pub a: Vec<f32>,
    pub b: Vec<f32>,
    pub alpha: Option<Vec<f32>>,
}

impl OklabPlanes {
    /// Create zero-initialized planes for the given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            l: vec![0.0; n],
            a: vec![0.0; n],
            b: vec![0.0; n],
            alpha: None,
        }
    }

    /// Create zero-initialized planes with an alpha channel.
    pub fn with_alpha(width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            l: vec![0.0; n],
            a: vec![0.0; n],
            b: vec![0.0; n],
            alpha: Some(vec![0.0; n]),
        }
    }

    /// Create planes using buffers borrowed from a [`FilterContext`].
    ///
    /// This avoids allocating new vectors when the context already has
    /// suitably-sized buffers from a previous pipeline run.
    pub fn from_ctx(ctx: &mut FilterContext, width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            l: ctx.take_f32(n),
            a: ctx.take_f32(n),
            b: ctx.take_f32(n),
            alpha: None,
        }
    }

    /// Create planes with alpha using buffers borrowed from a [`FilterContext`].
    pub fn from_ctx_with_alpha(ctx: &mut FilterContext, width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            l: ctx.take_f32(n),
            a: ctx.take_f32(n),
            b: ctx.take_f32(n),
            alpha: Some(ctx.take_f32(n)),
        }
    }

    /// Return all plane buffers to the [`FilterContext`] pool.
    ///
    /// After this call, the planes are empty and should not be used.
    pub fn return_to_ctx(self, ctx: &mut FilterContext) {
        ctx.return_f32(self.l);
        ctx.return_f32(self.a);
        ctx.return_f32(self.b);
        if let Some(alpha) = self.alpha {
            ctx.return_f32(alpha);
        }
    }

    /// Total number of pixels.
    #[inline]
    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Linear index for pixel at (x, y).
    #[inline]
    pub fn index(&self, x: u32, y: u32) -> usize {
        debug_assert!(x < self.width && y < self.height);
        (y as usize) * (self.width as usize) + (x as usize)
    }

    /// Row slice range for row y.
    #[inline]
    pub fn row_range(&self, y: u32) -> core::ops::Range<usize> {
        let start = (y as usize) * (self.width as usize);
        start..start + (self.width as usize)
    }
}
