use alloc::boxed::Box;
use alloc::string::String;
use core::fmt;

/// Result type with `whereat::At<PipeError>` as the error.
pub type PipeResult<T> = Result<T, whereat::At<PipeError>>;

/// Pipeline execution error.
#[derive(Debug)]
pub enum PipeError {
    /// Upstream source produced data in an unexpected format.
    FormatMismatch {
        expected: crate::PixelFormat,
        got: crate::PixelFormat,
    },
    /// Resize operation failed.
    Resize(String),
    /// Strip dimensions don't match expectations.
    DimensionMismatch(String),
    /// Resource limit exceeded (dimensions, pixel count, or memory).
    LimitExceeded(String),
    /// Operation cancelled via `enough::Stop`.
    Cancelled,
    /// Generic operation error.
    Op(String),
    /// Error from a codec or downstream crate, with preserved error type.
    Codec(Box<dyn core::error::Error + Send + Sync>),
}

impl fmt::Display for PipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatMismatch { expected, got } => {
                write!(f, "format mismatch: expected {expected}, got {got}")
            }
            Self::Resize(msg) => write!(f, "resize: {msg}"),
            Self::DimensionMismatch(msg) => write!(f, "dimension mismatch: {msg}"),
            Self::LimitExceeded(msg) => write!(f, "limit exceeded: {msg}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Op(msg) => write!(f, "operation: {msg}"),
            Self::Codec(e) => write!(f, "codec: {e}"),
        }
    }
}

impl core::error::Error for PipeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Codec(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<enough::StopReason> for PipeError {
    fn from(_: enough::StopReason) -> Self {
        Self::Cancelled
    }
}
