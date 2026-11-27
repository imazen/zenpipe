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
    /// Format recognized but the codec isn't compiled in (feature not enabled
    /// at build time). For a codec that IS compiled in but disabled at
    /// runtime via [`AllowedFormats`](crate::AllowedFormats), see
    /// [`DisabledFormat`](Self::DisabledFormat) instead.
    #[error("format {0:?} not supported (codec not compiled in)")]
    UnsupportedFormat(ImageFormat),
    /// Format doesn't support requested operation.
    #[error("format {format:?} does not support: {detail}")]
    UnsupportedOperation {
        format: ImageFormat,
        detail: &'static str,
    },
    /// Format's codec IS compiled in, but disabled at runtime by the
    /// [`AllowedFormats`](crate::AllowedFormats) registry for this request.
    /// For a codec that isn't compiled in at all, see
    /// [`UnsupportedFormat`](Self::UnsupportedFormat) instead.
    #[error("format {0:?} is disabled in the codec registry")]
    DisabledFormat(ImageFormat),
    /// Input validation failed.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Resource limit exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    /// Operation stopped via cancellation or deadline (`enough::StopReason`).
    #[error("operation stopped: {0}")]
    Stopped(enough::StopReason),
    /// Allocation failure.
    #[error("out of memory")]
    Oom,
    /// No suitable encoder found for auto-selection.
    #[error("no suitable encoder found for auto-selection")]
    NoSuitableEncoder,
    /// Color management error.
    #[cfg(feature = "cms")]
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

impl From<enough::StopReason> for CodecError {
    fn from(reason: enough::StopReason) -> Self {
        Self::Stopped(reason)
    }
}

/// Extract an [`enough::StopReason`] from a codec error chain if the failure
/// represents cooperative cancellation or a deadline timeout.
fn extract_stop_reason(error: &(dyn core::error::Error + 'static)) -> Option<enough::StopReason> {
    if let Some(ce) = zencodec::find_cause::<zencodec::CodecError>(error)
        && let zencodec::ErrorCategory::Stopped(reason) = ce.category()
    {
        return Some(reason);
    }
    None
}

// Conversion helpers for codec-specific errors
impl CodecError {
    /// Wrap a codec-specific error, lifting cooperative cancellation/timeouts
    /// (`zencodec::ErrorCategory::Stopped` or `enough::StopReason`) into
    /// [`CodecError::Stopped`].
    ///
    /// Use with `at!()` to capture location:
    /// ```ignore
    /// .map_err(|e| at!(CodecError::from_codec(ImageFormat::Png, e)))
    /// ```
    pub fn from_codec<E>(format: ImageFormat, error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        if let Some(reason) = extract_stop_reason(&error) {
            return CodecError::Stopped(reason);
        }
        CodecError::Codec {
            format,
            source: Box::new(error),
        }
    }

    /// Wrap a pre-boxed codec error (from dyn dispatch), lifting cooperative
    /// cancellation/timeouts into [`CodecError::Stopped`].
    pub fn from_codec_boxed(
        format: ImageFormat,
        source: Box<dyn core::error::Error + Send + Sync>,
    ) -> Self {
        if let Some(reason) = extract_stop_reason(source.as_ref()) {
            return CodecError::Stopped(reason);
        }
        CodecError::Codec { format, source }
    }
}
