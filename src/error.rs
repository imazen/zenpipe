//! Unified error types for codec operations.

use alloc::boxed::Box;
use alloc::string::String;
use core::fmt;

use crate::ImageFormat;

/// Unified error type for codec operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum CodecError {
    /// Format not recognized from magic bytes.
    UnrecognizedFormat,
    /// Format recognized but codec not compiled in or not enabled in registry.
    UnsupportedFormat(ImageFormat),
    /// Format doesn't support requested operation.
    UnsupportedOperation {
        format: ImageFormat,
        detail: &'static str,
    },
    /// Codec not enabled in the provided registry.
    DisabledFormat(ImageFormat),
    /// Input validation failed.
    InvalidInput(String),
    /// Resource limit exceeded.
    LimitExceeded(String),
    /// Operation cancelled via Stop token.
    Cancelled,
    /// Allocation failure.
    Oom,
    /// No suitable encoder found for auto-selection.
    NoSuitableEncoder,
    /// Color management error.
    #[cfg(feature = "moxcms")]
    ColorManagement(String),
    /// Underlying codec error with caller location.
    Codec {
        format: ImageFormat,
        source: Box<dyn core::error::Error + Send + Sync>,
        caller: &'static core::panic::Location<'static>,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::UnrecognizedFormat => write!(f, "unrecognized image format"),
            CodecError::UnsupportedFormat(format) => {
                write!(
                    f,
                    "format {:?} not supported (codec not compiled in)",
                    format
                )
            }
            CodecError::UnsupportedOperation { format, detail } => {
                write!(f, "format {:?} does not support: {}", format, detail)
            }
            CodecError::DisabledFormat(format) => {
                write!(f, "format {:?} is disabled in the codec registry", format)
            }
            CodecError::InvalidInput(msg) => write!(f, "invalid input: {}", msg),
            CodecError::LimitExceeded(msg) => write!(f, "limit exceeded: {}", msg),
            CodecError::Cancelled => write!(f, "operation cancelled"),
            CodecError::Oom => write!(f, "out of memory"),
            CodecError::NoSuitableEncoder => {
                write!(f, "no suitable encoder found for auto-selection")
            }
            #[cfg(feature = "moxcms")]
            CodecError::ColorManagement(msg) => write!(f, "color management error: {}", msg),
            CodecError::Codec {
                format,
                source,
                caller,
            } => {
                write!(f, "codec error ({:?}) at {}: {}", format, caller, source)
            }
        }
    }
}

impl core::error::Error for CodecError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            CodecError::Codec { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

// Conversion helpers for codec-specific errors
impl CodecError {
    /// Wrap a codec-specific error, capturing the caller's location.
    #[track_caller]
    pub fn from_codec<E>(format: ImageFormat, error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        CodecError::Codec {
            format,
            source: Box::new(error),
            caller: core::panic::Location::caller(),
        }
    }
}
