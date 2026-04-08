//! CLI error types and exit codes.

use std::process::ExitCode;

/// Exit codes per CLI-SPEC section 9.
pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_INPUT_ERROR: u8 = 1;
pub const EXIT_OPERATION_ERROR: u8 = 2;
pub const EXIT_OUTPUT_ERROR: u8 = 3;
pub const EXIT_PARTIAL_FAILURE: u8 = 4;

/// Unified CLI error.
#[derive(Debug)]
pub enum CliError {
    /// File not found, unsupported format, etc.
    Input(String),
    /// Invalid parameter, pipeline failure, etc.
    Operation(String),
    /// Write failed, disk full, etc.
    Output(String),
    /// Batch mode: some files failed.
    Partial { succeeded: usize, failed: usize },
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Input(_) => ExitCode::from(EXIT_INPUT_ERROR),
            Self::Operation(_) => ExitCode::from(EXIT_OPERATION_ERROR),
            Self::Output(_) => ExitCode::from(EXIT_OUTPUT_ERROR),
            Self::Partial { .. } => ExitCode::from(EXIT_PARTIAL_FAILURE),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(msg) => write!(f, "error: input: {msg}"),
            Self::Operation(msg) => write!(f, "error: operation: {msg}"),
            Self::Output(msg) => write!(f, "error: output: {msg}"),
            Self::Partial { succeeded, failed } => {
                write!(f, "batch: {succeeded} succeeded, {failed} failed")
            }
        }
    }
}
