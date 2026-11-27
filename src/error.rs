use alloc::boxed::Box;
use alloc::string::String;
use core::fmt;

/// Result type with `whereat::At<PipeError>` as the error.
pub type PipeResult<T> = Result<T, whereat::At<PipeError>>;

/// Extract the [`enough::StopReason`] from a codec failure that represents
/// cooperative cancellation or a deadline timeout (either
/// [`zencodec::ErrorCategory::Stopped`] or a lifted `zencodecs::CodecError::Stopped`
/// anywhere in its cause chain), which zenpipe reports as [`PipeError::Stopped`].
pub(crate) fn codec_stop_reason(
    error: &(dyn core::error::Error + 'static),
) -> Option<enough::StopReason> {
    #[cfg(feature = "zennode")]
    if let Some(zencodecs::CodecError::Stopped(reason)) =
        zencodec::find_cause::<zencodecs::CodecError>(error)
    {
        return Some(*reason);
    }
    if let Some(ce) = zencodec::find_cause::<zencodec::CodecError>(error)
        && let zencodec::ErrorCategory::Stopped(reason) = ce.category()
    {
        return Some(reason);
    }
    None
}

/// Pipeline execution error.
#[derive(Debug)]
#[non_exhaustive]
pub enum PipeError {
    /// Upstream source produced data in an unexpected format.
    FormatMismatch {
        expected: Box<crate::PixelFormat>,
        got: Box<crate::PixelFormat>,
    },
    /// Resize operation failed.
    Resize(String),
    /// Strip dimensions don't match expectations.
    DimensionMismatch(String),
    /// Resource limit exceeded (dimensions, pixel count, or memory).
    LimitExceeded(String),
    /// Operation stopped via cancellation or deadline (`enough::StopReason`).
    Stopped(enough::StopReason),
    /// Generic operation error.
    Op(String),
    /// Error from a codec or downstream crate, with preserved error type.
    Codec(Box<dyn core::error::Error + Send + Sync>),
}

impl fmt::Display for PipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatMismatch { expected, got } => {
                write!(f, "format mismatch: expected {expected:?}, got {got:?}")
            }
            Self::Resize(msg) => write!(f, "resize: {msg}"),
            Self::DimensionMismatch(msg) => write!(f, "dimension mismatch: {msg}"),
            Self::LimitExceeded(msg) => write!(f, "limit exceeded: {msg}"),
            Self::Stopped(reason) => write!(f, "operation stopped: {reason}"),
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
    fn from(reason: enough::StopReason) -> Self {
        Self::Stopped(reason)
    }
}
