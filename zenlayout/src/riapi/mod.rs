//! RIAPI query string parsing and pipeline construction.
//!
//! Parses URL query strings like `?w=800&h=600&mode=crop` and produces
//! zenlayout [`Pipeline`](crate::Pipeline) objects for layout computation.
//!
//! # Example
//!
//! ```
//! use zenlayout::riapi;
//!
//! let result = riapi::parse("w=800&h=600&mode=crop&scale=both");
//! assert!(result.warnings.is_empty());
//!
//! let pipeline = result.instructions
//!     .to_pipeline(4000, 3000, None)
//!     .expect("valid pipeline");
//!
//! let (ideal, _request) = pipeline.plan().expect("valid layout");
//! assert_eq!(ideal.layout.resize_to.width, 800);
//! assert_eq!(ideal.layout.resize_to.height, 600);
//! ```
//!
//! # Non-layout parameters
//!
//! Keys not relevant to geometry (format, quality, effects) are preserved
//! in [`Instructions::extras()`] for downstream consumers without generating
//! warnings. Only truly unrecognized keys produce [`ParseWarning::KeyNotRecognized`].

mod color;
mod convert;
pub mod instructions;
mod parse;

pub use color::parse_color;
pub use instructions::{Anchor1D, CFocus, FitMode, Instructions, ScaleMode};

use alloc::string::String;
use alloc::vec::Vec;

/// Result of parsing a RIAPI query string.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ParseResult {
    /// Parsed layout instructions.
    pub instructions: Instructions,
    /// Non-fatal parse warnings.
    pub warnings: Vec<ParseWarning>,
}

/// Non-fatal warning from query string parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseWarning {
    /// A key appeared more than once (last value wins).
    DuplicateKey { key: String, value: String },
    /// A key was not recognized as either a layout or known non-layout parameter.
    KeyNotRecognized { key: String, value: String },
    /// A key was recognized but its value could not be parsed.
    ValueInvalid {
        key: &'static str,
        value: String,
        reason: &'static str,
    },
}

/// Parse a RIAPI query string (with or without leading `?`).
///
/// Returns parsed instructions and any non-fatal warnings.
pub fn parse(query: &str) -> ParseResult {
    let (instructions, warnings) = parse::parse_query(query);
    ParseResult {
        instructions,
        warnings,
    }
}
