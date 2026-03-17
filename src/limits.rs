//! Resource limits for pipeline operations.
//!
//! [`Limits`] prevents memory-bomb attacks by rejecting images that exceed
//! configured dimension or pixel count thresholds before allocating.

use crate::error::PipeError;
use crate::format::PixelFormat;

/// Resource limits for pipeline operations.
///
/// Check these before any allocation proportional to image dimensions
/// (materialize, tee, orient, resize output). All fields are optional —
/// `None` means no limit.
///
/// # Example
///
/// ```
/// use zenpipe::Limits;
///
/// let limits = Limits::default()
///     .with_max_pixels(100_000_000)   // 100 megapixels
///     .with_max_width(16384)
///     .with_max_height(16384);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// Maximum total pixel count (width × height).
    pub max_pixels: Option<u64>,
    /// Maximum estimated memory in bytes for a single buffer allocation.
    pub max_memory_bytes: Option<u64>,
    /// Maximum image width in pixels.
    pub max_width: Option<u32>,
    /// Maximum image height in pixels.
    pub max_height: Option<u32>,
}

impl Limits {
    /// No limits — everything is allowed.
    pub const NONE: Self = Self {
        max_pixels: None,
        max_memory_bytes: None,
        max_width: None,
        max_height: None,
    };

    /// Server-safe defaults: 100 megapixels, 16384 max dimension, 4 GB memory.
    pub const SERVER: Self = Self {
        max_pixels: Some(100_000_000),
        max_memory_bytes: Some(4 * 1024 * 1024 * 1024),
        max_width: Some(16384),
        max_height: Some(16384),
    };

    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = Some(max);
        self
    }

    pub fn with_max_memory_bytes(mut self, max: u64) -> Self {
        self.max_memory_bytes = Some(max);
        self
    }

    pub fn with_max_width(mut self, max: u32) -> Self {
        self.max_width = Some(max);
        self
    }

    pub fn with_max_height(mut self, max: u32) -> Self {
        self.max_height = Some(max);
        self
    }

    /// Check whether an image of the given dimensions and format is within limits.
    ///
    /// Returns `Ok(())` if within limits, `Err(PipeError::LimitExceeded)` otherwise.
    pub fn check(&self, width: u32, height: u32, format: PixelFormat) -> Result<(), PipeError> {
        if let Some(max_w) = self.max_width {
            if width > max_w {
                return Err(PipeError::LimitExceeded(alloc::format!(
                    "width {width} exceeds max {max_w}"
                )));
            }
        }
        if let Some(max_h) = self.max_height {
            if height > max_h {
                return Err(PipeError::LimitExceeded(alloc::format!(
                    "height {height} exceeds max {max_h}"
                )));
            }
        }

        let pixels = width as u64 * height as u64;
        if let Some(max_px) = self.max_pixels {
            if pixels > max_px {
                return Err(PipeError::LimitExceeded(alloc::format!(
                    "pixel count {pixels} exceeds max {max_px}"
                )));
            }
        }

        if let Some(max_mem) = self.max_memory_bytes {
            let mem = pixels * format.bytes_per_pixel() as u64;
            if mem > max_mem {
                return Err(PipeError::LimitExceeded(alloc::format!(
                    "estimated memory {mem} bytes exceeds max {max_mem}"
                )));
            }
        }

        Ok(())
    }
}
