//! Quality preset to per-format quality mapping.

use zc::ImageFormat;

/// Quality presets with approximate perceptual equivalence across formats.
///
/// Each preset maps to format-specific quality values that produce
/// similar perceptual quality. The `Custom` variant allows explicit
/// quality values (0-100) that are passed through directly.
///
/// | Preset       | JPEG | WebP | AVIF | JXL (dist) | PNG  | GIF  |
/// |-------------|------|------|------|------------|------|------|
/// | Lossless    | 100  | lossless | lossless | 0.0 | lossless | as-is |
/// | NearLossless| 97   | 95   | 95   | 0.5  | lossless | as-is |
/// | HighQuality | 90   | 90   | 85   | 1.0  | lossless | as-is |
/// | Balanced    | 80   | 80   | 70   | 2.0  | lossless | as-is |
/// | SmallFile   | 60   | 60   | 45   | 4.0  | lossless | as-is |
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum QualityPreset {
    /// Lossless encoding. If the target format doesn't support lossless,
    /// uses maximum quality.
    Lossless,
    /// Visually indistinguishable from source.
    NearLossless,
    /// High quality, small artifacts acceptable.
    HighQuality,
    /// Good balance of quality and file size.
    Balanced,
    /// Prioritize small file size.
    SmallFile,
    /// Explicit quality value (0-100), passed through to the encoder.
    Custom(f32),
}

impl QualityPreset {
    /// Map this preset to a (quality, lossless) pair for the given format.
    ///
    /// Returns `(Some(quality), false)` for lossy, `(None, true)` for lossless.
    /// For formats that are always lossless (PNG, GIF), returns `(None, true)`
    /// regardless of preset.
    pub(crate) fn for_format(self, format: ImageFormat) -> (Option<f32>, bool) {
        // PNG and GIF are always lossless — ignore quality settings
        if matches!(format, ImageFormat::Png | ImageFormat::Gif) {
            return (None, true);
        }

        match self {
            QualityPreset::Lossless => {
                if format.supports_lossless() {
                    (None, true)
                } else {
                    // JPEG doesn't support lossless — use max quality
                    (Some(100.0), false)
                }
            }
            QualityPreset::NearLossless => {
                let q = match format {
                    ImageFormat::Jpeg => 97.0,
                    ImageFormat::WebP => 95.0,
                    ImageFormat::Avif => 95.0,
                    ImageFormat::Jxl => 99.5, // distance 0.5
                    _ => 97.0,
                };
                (Some(q), false)
            }
            QualityPreset::HighQuality => {
                let q = match format {
                    ImageFormat::Jpeg => 90.0,
                    ImageFormat::WebP => 90.0,
                    ImageFormat::Avif => 85.0,
                    ImageFormat::Jxl => 99.0, // distance 1.0
                    _ => 90.0,
                };
                (Some(q), false)
            }
            QualityPreset::Balanced => {
                let q = match format {
                    ImageFormat::Jpeg => 80.0,
                    ImageFormat::WebP => 80.0,
                    ImageFormat::Avif => 70.0,
                    ImageFormat::Jxl => 98.0, // distance 2.0
                    _ => 80.0,
                };
                (Some(q), false)
            }
            QualityPreset::SmallFile => {
                let q = match format {
                    ImageFormat::Jpeg => 60.0,
                    ImageFormat::WebP => 60.0,
                    ImageFormat::Avif => 45.0,
                    ImageFormat::Jxl => 96.0, // distance 4.0
                    _ => 60.0,
                };
                (Some(q), false)
            }
            QualityPreset::Custom(q) => (Some(q), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossless_jpeg_gives_max_quality() {
        let (q, lossless) = QualityPreset::Lossless.for_format(ImageFormat::Jpeg);
        assert_eq!(q, Some(100.0));
        assert!(!lossless);
    }

    #[test]
    fn lossless_webp_is_lossless() {
        let (q, lossless) = QualityPreset::Lossless.for_format(ImageFormat::WebP);
        assert_eq!(q, None);
        assert!(lossless);
    }

    #[test]
    fn lossless_png_always_lossless() {
        let (q, lossless) = QualityPreset::Lossless.for_format(ImageFormat::Png);
        assert_eq!(q, None);
        assert!(lossless);
    }

    #[test]
    fn balanced_jpeg() {
        let (q, lossless) = QualityPreset::Balanced.for_format(ImageFormat::Jpeg);
        assert_eq!(q, Some(80.0));
        assert!(!lossless);
    }

    #[test]
    fn balanced_avif() {
        let (q, lossless) = QualityPreset::Balanced.for_format(ImageFormat::Avif);
        assert_eq!(q, Some(70.0));
        assert!(!lossless);
    }

    #[test]
    fn custom_passthrough() {
        let (q, lossless) = QualityPreset::Custom(42.0).for_format(ImageFormat::Jpeg);
        assert_eq!(q, Some(42.0));
        assert!(!lossless);
    }

    #[test]
    fn gif_always_lossless() {
        // GIF is always lossless regardless of preset
        for preset in [
            QualityPreset::SmallFile,
            QualityPreset::Balanced,
            QualityPreset::HighQuality,
            QualityPreset::Custom(50.0),
        ] {
            let (q, lossless) = preset.for_format(ImageFormat::Gif);
            assert_eq!(q, None);
            assert!(lossless);
        }
    }

    #[test]
    fn small_file_webp() {
        let (q, lossless) = QualityPreset::SmallFile.for_format(ImageFormat::WebP);
        assert_eq!(q, Some(60.0));
        assert!(!lossless);
    }

    #[test]
    fn near_lossless_avif() {
        let (q, lossless) = QualityPreset::NearLossless.for_format(ImageFormat::Avif);
        assert_eq!(q, Some(95.0));
        assert!(!lossless);
    }
}
