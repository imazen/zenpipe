//! Pixel format descriptor — re-exported from [`zenpixels_convert`].
//!
//! [`PixelFormat`] is a type alias for [`zenpixels_convert::PixelDescriptor`],
//! which carries color primaries, transfer function, alpha mode, and channel
//! layout. This replaces the old 4-variant enum with a composable descriptor
//! that supports P3, BT.2020, HDR (PQ/HLG), and arbitrary channel types.

pub use zenpixels_convert::{
    AlphaMode, ChannelLayout, ChannelType, PixelDescriptor, TransferFunction,
};

/// Type alias preserving the old name. All zenpipe APIs use this type.
pub type PixelFormat = PixelDescriptor;

// =========================================================================
// Well-known format constants (backward compatibility + common formats)
// =========================================================================

/// 4 bytes/pixel, sRGB transfer, straight alpha. Standard decode/encode format.
pub const RGBA8_SRGB: PixelFormat = PixelDescriptor::RGBA8_SRGB;

/// 16 bytes/pixel (4×f32), linear light, premultiplied alpha.
/// Working format for resize and compositing.
pub const RGBAF32_LINEAR_PREMUL: PixelFormat = PixelDescriptor::new(
    ChannelType::F32,
    ChannelLayout::Rgba,
    Some(AlphaMode::Premultiplied),
    TransferFunction::Linear,
);

/// 16 bytes/pixel (4×f32), linear light, straight alpha.
pub const RGBAF32_LINEAR: PixelFormat = PixelDescriptor::RGBAF32_LINEAR;

/// 16 bytes/pixel (4×f32), sRGB transfer, straight alpha.
/// Identity mode — no gamma conversion, just normalized to 0..1.
pub const RGBAF32_SRGB: PixelFormat = PixelDescriptor::new(
    ChannelType::F32,
    ChannelLayout::Rgba,
    Some(AlphaMode::Straight),
    TransferFunction::Srgb,
);

// =========================================================================
// Extension trait for row_bytes (convenience for strip calculations)
// =========================================================================

/// Extension methods for [`PixelDescriptor`] used throughout zenpipe.
pub trait PixelFormatExt {
    /// Row stride in bytes for the given width (tightly packed, no padding).
    fn row_bytes(self, width: u32) -> usize;
}

impl PixelFormatExt for PixelDescriptor {
    #[inline]
    fn row_bytes(self, width: u32) -> usize {
        width as usize * self.bytes_per_pixel()
    }
}
