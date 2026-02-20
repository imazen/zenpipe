//! Output path resolution with format-aware extension changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use zencodecs::ImageFormat;

/// Resolved output configuration.
pub struct OutputConfig {
    pub target_dir: Option<PathBuf>,
    pub target_file: Option<PathBuf>,
    pub in_place: bool,
    pub suffix: String,
    pub force: bool,
    pub dry_run: bool,
    /// Target format (used for extension changes during transcode).
    pub target_format: Option<ImageFormat>,
}

impl OutputConfig {
    /// Create from CLI args.
    pub fn new(
        output: Option<&str>,
        in_place: bool,
        suffix: &str,
        force: bool,
        dry_run: bool,
        target_format: Option<ImageFormat>,
    ) -> anyhow::Result<Self> {
        if in_place && !force {
            bail!("--in-place requires --force to confirm overwriting originals");
        }

        let (target_dir, target_file) = match output {
            Some(o) => {
                let path = PathBuf::from(o);
                if o.ends_with('/') || o.ends_with('\\') || path.is_dir() {
                    (Some(path), None)
                } else {
                    (None, Some(path))
                }
            }
            None => (None, None),
        };

        Ok(Self {
            target_dir,
            target_file,
            in_place,
            suffix: suffix.to_string(),
            force,
            dry_run,
            target_format,
        })
    }

    /// Resolve the output path for a given input file.
    pub fn resolve(&self, input: &Path, input_count: usize) -> anyhow::Result<PathBuf> {
        // --in-place: write back to input path
        if self.in_place {
            return Ok(input.to_path_buf());
        }

        // -o file: only valid for single-file input
        if let Some(ref target) = self.target_file {
            if input_count > 1 {
                bail!("-o with a file path only works for a single input file (got {input_count})");
            }
            return Ok(target.clone());
        }

        // -o dir/: place output in target directory with same filename
        if let Some(ref dir) = self.target_dir {
            let filename = self.output_filename(input);
            return Ok(dir.join(filename));
        }

        // Default: same directory as input, with suffix and possibly new extension
        let parent = input.parent().unwrap_or(Path::new("."));
        let filename = self.output_filename(input);
        Ok(parent.join(filename))
    }

    /// Compute the output filename (stem + suffix + extension).
    fn output_filename(&self, input: &Path) -> String {
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");

        let ext = self.output_extension(input);

        if self.suffix.is_empty() {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}{}.{ext}", self.suffix)
        }
    }

    /// Determine the output extension based on target format or input extension.
    fn output_extension(&self, input: &Path) -> String {
        if let Some(fmt) = self.target_format {
            // Use the primary extension for the target format
            fmt.extensions().first().unwrap_or(&"bin").to_string()
        } else {
            // Keep original extension
            input
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin")
                .to_string()
        }
    }

    /// Check if the output path is writable (won't clobber without --force).
    pub fn check_writable(&self, input: &Path, output: &Path) -> anyhow::Result<()> {
        if self.dry_run {
            return Ok(());
        }

        // Same-as-input without --in-place is an error
        if !self.in_place {
            if let (Ok(ci), Ok(co)) = (input.canonicalize(), output.canonicalize()) {
                if ci == co {
                    bail!(
                        "output would overwrite input: {}\nUse --in-place --force to confirm",
                        input.display()
                    );
                }
            }
        }

        // Existing output without --force
        if output.exists() && !self.force && !self.in_place {
            bail!(
                "output already exists: {}\nUse --force to overwrite",
                output.display()
            );
        }

        Ok(())
    }

    /// Create parent directories for the output path.
    pub fn ensure_parent(output: &Path) -> anyhow::Result<()> {
        if let Some(parent) = output.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating directory: {}", parent.display()))?;
            }
        }
        Ok(())
    }
}
