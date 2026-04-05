//! Glob expansion and output path template handling.

use std::path::{Path, PathBuf};

use crate::error::CliError;

/// Expand a glob pattern to a list of matching file paths.
pub fn expand_glob(pattern: &str) -> Result<Vec<PathBuf>, CliError> {
    let paths: Vec<PathBuf> = glob::glob(pattern)
        .map_err(|e| CliError::Input(format!("invalid glob pattern '{pattern}': {e}")))?
        .filter_map(|entry| entry.ok())
        .filter(|p| p.is_file())
        .collect();
    Ok(paths)
}

/// Expand an output template with placeholders.
///
/// Placeholders:
///   {name}  — input filename without extension
///   {ext}   — input extension
///   {dir}   — input directory
///   {n}     — sequence number (0-based)
///   {w}     — output width (not known here, replaced later)
///   {h}     — output height (not known here, replaced later)
pub fn expand_template(template: &str, input: &Path, index: usize) -> String {
    let name = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let ext = input
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let dir = input
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    template
        .replace("{name}", &name)
        .replace("{ext}", &ext)
        .replace("{dir}", &dir)
        .replace("{n}", &index.to_string())
}
