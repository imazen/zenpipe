/// Pixel format describing how strip data is laid out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4 bytes/pixel, sRGB transfer, straight alpha. Standard decode/encode format.
    Rgba8,
    /// 16 bytes/pixel (4×f32), linear light, premultiplied alpha.
    /// Working format for resize and compositing.
    Rgbaf32LinearPremul,
    /// 16 bytes/pixel (4×f32), linear light, straight alpha.
    Rgbaf32Linear,
    /// 16 bytes/pixel (4×f32), sRGB transfer, straight alpha.
    /// Identity mode — no gamma conversion, just normalized to 0..1.
    Rgbaf32Srgb,
}

impl PixelFormat {
    /// Bytes per pixel for this format.
    #[inline]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgbaf32LinearPremul | Self::Rgbaf32Linear | Self::Rgbaf32Srgb => 16,
        }
    }

    /// Whether this format uses f32 channels.
    #[inline]
    pub const fn is_f32(self) -> bool {
        !matches!(self, Self::Rgba8)
    }

    /// Row stride in bytes for the given width.
    #[inline]
    pub const fn row_bytes(self, width: u32) -> usize {
        width as usize * self.bytes_per_pixel()
    }
}
