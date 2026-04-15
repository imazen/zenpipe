//! File expansion, deduplication, sorting, and batch reporting.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;


/// Expand input patterns into a deduplicated, sorted list of image files.
///
/// Handles:
/// - Glob patterns (containing `*`, `?`, `[`)
/// - Plain file paths
/// - Directories (recursive image discovery)
///
/// Results are deduplicated by canonical path and sorted by file size
/// descending for better parallel load balancing.
pub fn expand_inputs(patterns: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();

    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            for entry in glob::glob(pattern)? {
                let path = entry?;
                if path.is_file() && is_image(&path) {
                    if let Ok(canonical) = path.canonicalize() {
                        if seen.insert(canonical) {
                            files.push(path);
                        }
                    }
                }
            }
        } else {
            let path = PathBuf::from(pattern);
            if path.is_dir() {
                for_each_image_in_dir(&path, &mut seen, &mut files);
            } else if path.is_file() {
                if let Ok(canonical) = path.canonicalize() {
                    if seen.insert(canonical) {
                        files.push(path);
                    }
                }
            } else {
                anyhow::bail!("not a file or directory: {}", path.display());
            }
        }
    }

    // Sort by file size descending for better parallel load balancing
    files.sort_by(|a, b| {
        let size_a = a.metadata().map(|m| m.len()).unwrap_or(0);
        let size_b = b.metadata().map(|m| m.len()).unwrap_or(0);
        size_b.cmp(&size_a)
    });

    Ok(files)
}

/// Check if a file path has a recognized image extension.
pub fn is_image(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e,
        None => return false,
    };
    zencodec::ImageFormatRegistry::common().from_extension(ext).is_some()
}

/// Recursively find image files in a directory.
fn for_each_image_in_dir(dir: &Path, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            for_each_image_in_dir(&path, seen, files);
        } else if path.is_file() && is_image(&path) {
            if let Ok(canonical) = path.canonicalize() {
                if seen.insert(canonical) {
                    files.push(path);
                }
            }
        }
    }
}

/// Result of processing a single file.
#[derive(Debug)]
pub struct FileResult {
    pub input_path: PathBuf,
    pub input_size: u64,
    pub output_size: Option<u64>,
    pub output_path: Option<PathBuf>,
    pub skipped: bool,
    pub error: Option<String>,
    pub duration: Duration,
}

/// Accumulated batch processing summary.
#[derive(Debug, Default)]
pub struct BatchSummary {
    pub results: Vec<FileResult>,
}

impl BatchSummary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, result: FileResult) {
        self.results.push(result);
    }

    pub fn total_input_size(&self) -> u64 {
        self.results.iter().map(|r| r.input_size).sum()
    }

    pub fn total_output_size(&self) -> u64 {
        self.results.iter().filter_map(|r| r.output_size).sum()
    }

    pub fn success_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.error.is_none() && !r.skipped)
            .count()
    }

    pub fn skip_count(&self) -> usize {
        self.results.iter().filter(|r| r.skipped).count()
    }

    pub fn error_count(&self) -> usize {
        self.results.iter().filter(|r| r.error.is_some()).count()
    }

    /// Print a human-readable summary table.
    pub fn print_report(&self) {
        if self.results.is_empty() {
            println!("No files processed.");
            return;
        }

        // Header
        println!(
            "{:<40} {:>10} {:>10} {:>8} {:>8}",
            "File", "Input", "Output", "Change", "Time"
        );
        println!("{}", "-".repeat(80));

        for r in &self.results {
            let name = r
                .input_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let name = if name.len() > 38 {
                format!("..{}", &name[name.len() - 36..])
            } else {
                name.to_string()
            };

            if let Some(err) = &r.error {
                println!("{:<40} {:>10} {}", name, format_size(r.input_size), err);
            } else if r.skipped {
                println!(
                    "{:<40} {:>10} {:>10}",
                    name,
                    format_size(r.input_size),
                    "skipped"
                );
            } else if let Some(out_size) = r.output_size {
                let change = if r.input_size > 0 {
                    let pct = (out_size as f64 - r.input_size as f64) / r.input_size as f64 * 100.0;
                    format!("{:+.1}%", pct)
                } else {
                    "N/A".to_string()
                };
                let time_ms = r.duration.as_millis();
                let time_str = if time_ms >= 1000 {
                    format!("{:.1}s", time_ms as f64 / 1000.0)
                } else {
                    format!("{}ms", time_ms)
                };
                println!(
                    "{:<40} {:>10} {:>10} {:>8} {:>8}",
                    name,
                    format_size(r.input_size),
                    format_size(out_size),
                    change,
                    time_str,
                );
            }
        }

        // Summary
        println!("{}", "-".repeat(80));
        let total_in = self.total_input_size();
        let total_out = self.total_output_size();
        let change = if total_in > 0 {
            let pct = (total_out as f64 - total_in as f64) / total_in as f64 * 100.0;
            format!("{:+.1}%", pct)
        } else {
            "N/A".to_string()
        };
        println!(
            "{} processed, {} skipped, {} errors | {} -> {} ({})",
            self.success_count(),
            self.skip_count(),
            self.error_count(),
            format_size(total_in),
            format_size(total_out),
            change,
        );
    }

    /// Write results as CSV.
    pub fn write_csv(&self, path: &Path) -> anyhow::Result<()> {
        let mut f = std::fs::File::create(path)?;
        writeln!(
            f,
            "input,input_size,output,output_size,change_pct,duration_ms,status"
        )?;
        for r in &self.results {
            let status = if r.error.is_some() {
                "error"
            } else if r.skipped {
                "skipped"
            } else {
                "ok"
            };
            let out_path = r
                .output_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let out_size = r.output_size.unwrap_or(0);
            let change = if r.input_size > 0 && r.output_size.is_some() {
                (out_size as f64 - r.input_size as f64) / r.input_size as f64 * 100.0
            } else {
                0.0
            };
            writeln!(
                f,
                "{},{},{},{},{:.1},{},{}",
                r.input_path.display(),
                r.input_size,
                out_path,
                out_size,
                change,
                r.duration.as_millis(),
                status,
            )?;
        }
        Ok(())
    }
}

/// Format a byte size into a human-readable string.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
