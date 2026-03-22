//! Unified error types for codec operations.
//!
//! Uses `whereat::At<CodecError>` for production error location tracing.

use alloc::boxed::Box;
use alloc::string::String;

use crate::ImageFormat;

/// Result type alias using `At<CodecError>` for automatic location tracking.
pub type Result<T> = core::result::Result<T, whereat::At<CodecError>>;

/// Unified error type for codec operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CodecError {
    /// Format not recognized from magic bytes.
    #[error("unrecognized image format")]
    UnrecognizedFormat,
    /// Format recognized but codec not compiled in or not enabled in registry.
    #[error("format {0:?} not supported (codec not compiled in)")]
    UnsupportedFormat(ImageFormat),
    /// Format doesn't support requested operation.
    #[error("format {format:?} does not support: {detail}")]
    UnsupportedOperation {
        format: ImageFormat,
        detail: &'static str,
    },
    /// Codec not enabled in the provided registry.
    #[error("format {0:?} is disabled in the codec registry")]
    DisabledFormat(ImageFormat),
    /// Input validation failed.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Resource limit exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    /// Operation cancelled via Stop token.
    #[error("operation cancelled")]
    Cancelled,
    /// Allocation failure.
    #[error("out of memory")]
    Oom,
    /// No suitable encoder found for auto-selection.
    #[error("no suitable encoder found for auto-selection")]
    NoSuitableEncoder,
    /// Color management error.
    #[cfg(feature = "moxcms")]
    #[error("color management error: {0}")]
    ColorManagement(String),
    /// Underlying codec error.
    #[error("codec error ({format:?}): {source}")]
    Codec {
        format: ImageFormat,
        #[source]
        source: Box<dyn core::error::Error + Send + Sync>,
    },
}

// Conversion helpers for codec-specific errors
impl CodecError {
    /// Wrap a codec-specific error.
    ///
    /// Use with `at!()` to capture location:
    /// ```ignore
    /// .map_err(|e| at!(CodecError::from_codec(ImageFormat::Png, e)))
    /// ```
    pub fn from_codec<E>(format: ImageFormat, error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        CodecError::Codec {
            format,
            source: Box::new(error),
        }
    }

    /// Wrap a pre-boxed codec error (from dyn dispatch).
    pub fn from_codec_boxed(
        format: ImageFormat,
        source: Box<dyn core::error::Error + Send + Sync>,
    ) -> Self {
        CodecError::Codec { format, source }
    }
}
