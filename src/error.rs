use alloc::string::String;
use core::fmt;

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
    /// Generic operation error.
    Op(String),
}

impl fmt::Display for PipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatMismatch { expected, got } => {
                write!(f, "format mismatch: expected {expected:?}, got {got:?}")
            }
            Self::Resize(msg) => write!(f, "resize: {msg}"),
            Self::DimensionMismatch(msg) => write!(f, "dimension mismatch: {msg}"),
            Self::Op(msg) => write!(f, "operation: {msg}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PipeError {}
