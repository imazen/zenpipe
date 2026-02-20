//! zcimg — multi-format image processor.
//!
//! Unified decode → orient → resize → encode pipeline for transcoding,
//! re-encoding, and inspecting images across all zencodecs-supported formats.

mod batch;
mod info;
mod metadata;
mod output;
mod process;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Subcommand)]
enum Command {
    /// Transcode, re-encode, or optimize images (default for bare files).
    Process(Box<ProcessArgs>),

    /// Probe and display image metadata without decoding.
    Info(InfoArgs),
}

/// Arguments for the `process` subcommand.
#[derive(Parser, Debug)]
pub struct ProcessArgs {
    /// Input files or glob patterns.
    #[arg(required = true)]
    pub files: Vec<String>,

    // --- Output ---
    /// Output file or directory (dir/ with trailing slash for batch).
    #[arg(short, long)]
    pub output: Option<String>,

    /// Overwrite input files in place (requires --force).
    #[arg(long)]
    pub in_place: bool,

    /// Filename suffix before extension (default: none).
    #[arg(long, default_value = "")]
    pub suffix: String,

    /// Allow overwriting existing files.
    #[arg(long)]
    pub force: bool,

    /// Show what would be done without writing files.
    #[arg(long)]
    pub dry_run: bool,

    // --- Format ---
    /// Target output format.
    #[arg(short, long, value_enum)]
    pub format: Option<FormatArg>,

    // --- Quality ---
    /// Quality (0-100). Overrides --preset.
    #[arg(short, long)]
    pub quality: Option<f32>,

    /// Quality preset.
    #[arg(long, value_enum)]
    pub preset: Option<PresetArg>,

    /// Shorthand for --preset lossless.
    #[arg(long)]
    pub lossless: bool,

    /// Encoding effort / speed tradeoff (codec-specific).
    #[arg(long)]
    pub effort: Option<u32>,

    // --- Sizing ---
    /// Target width in pixels.
    #[arg(short = 'w', long)]
    pub width: Option<u32>,

    /// Target height in pixels.
    #[arg(short = 'H', long)]
    pub height: Option<u32>,

    /// Target size as WxH (e.g., 800x600).
    #[arg(long)]
    pub size: Option<String>,

    /// Fit mode (default: scale-down).
    #[arg(long, value_enum, default_value = "scale-down")]
    pub fit: FitMode,

    // --- Transforms ---
    /// Rotate by degrees (90, 180, 270).
    #[arg(long)]
    pub rotate: Option<u16>,

    /// Flip direction.
    #[arg(long, value_enum)]
    pub flip: Option<FlipArg>,

    /// Auto-orient from EXIF (default: on).
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub auto_orient: bool,

    // --- Metadata ---
    /// Strip all metadata (ICC, EXIF, XMP).
    #[arg(long)]
    pub strip_all: bool,

    /// Strip ICC profile only.
    #[arg(long)]
    pub strip_icc: bool,

    /// Strip EXIF only.
    #[arg(long)]
    pub strip_exif: bool,

    /// Strip XMP only.
    #[arg(long)]
    pub strip_xmp: bool,

    /// Strip everything except ICC profile.
    #[arg(long)]
    pub preserve_icc: bool,

    // --- Batch ---
    /// Number of parallel workers (default: CPU count).
    #[arg(short = 'j', long)]
    pub jobs: Option<usize>,

    /// Print summary report after batch processing.
    #[arg(long)]
    pub report: bool,

    /// Write CSV report to file.
    #[arg(long)]
    pub csv: Option<PathBuf>,

    /// Skip writing output if it would be larger than input.
    #[arg(long)]
    pub skip_if_larger: bool,
}

/// Arguments for the `info` subcommand.
#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// Input files or glob patterns.
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Parse and display full metadata (EXIF tags, ICC profile info, XMP, CICP names).
    #[arg(long, short = 'm')]
    pub metadata: bool,
}

/// Target image format.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FormatArg {
    Jpeg,
    Webp,
    Png,
    Gif,
    Avif,
    Jxl,
}

impl FormatArg {
    pub fn to_image_format(self) -> zencodecs::ImageFormat {
        match self {
            FormatArg::Jpeg => zencodecs::ImageFormat::Jpeg,
            FormatArg::Webp => zencodecs::ImageFormat::WebP,
            FormatArg::Png => zencodecs::ImageFormat::Png,
            FormatArg::Gif => zencodecs::ImageFormat::Gif,
            FormatArg::Avif => zencodecs::ImageFormat::Avif,
            FormatArg::Jxl => zencodecs::ImageFormat::Jxl,
        }
    }
}

/// Quality preset.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PresetArg {
    Lossless,
    NearLossless,
    High,
    Balanced,
    Small,
}

/// Fit mode for resize operations.
#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum FitMode {
    /// Fit within bounds, never upscale (default).
    #[default]
    ScaleDown,
    /// Fit within bounds, may upscale.
    Contain,
    /// Fill bounds, cropping excess.
    Cover,
    /// Fit within bounds, pad to fill.
    Pad,
    /// Stretch to exact dimensions.
    Fill,
}

/// Flip direction.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FlipArg {
    /// Flip horizontally.
    H,
    /// Flip vertically.
    V,
}

impl ProcessArgs {
    /// Resolve the target format from --format, -o extension, or None (same as input).
    pub fn resolve_format(&self) -> Option<zencodecs::ImageFormat> {
        // Explicit --format takes priority
        if let Some(fmt) = self.format {
            return Some(fmt.to_image_format());
        }

        // Auto-detect from -o extension
        if let Some(ref out) = self.output {
            let path = std::path::Path::new(out);
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if let Some(fmt) = zencodecs::ImageFormat::from_extension(ext) {
                    return Some(fmt);
                }
            }
        }

        // None = same format as input
        None
    }

    /// Parse --size WxH into (width, height).
    pub fn resolve_dimensions(&self) -> anyhow::Result<Option<(u32, u32)>> {
        if let Some(ref size) = self.size {
            let parts: Vec<&str> = size.split('x').collect();
            if parts.len() != 2 {
                anyhow::bail!("--size must be WxH (e.g., 800x600), got: {}", size);
            }
            let w: u32 = parts[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid width in --size: {}", parts[0]))?;
            let h: u32 = parts[1]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid height in --size: {}", parts[1]))?;
            return Ok(Some((w, h)));
        }

        match (self.width, self.height) {
            (Some(w), Some(h)) => Ok(Some((w, h))),
            (Some(w), None) => Ok(Some((w, 0))), // height auto-calculated
            (None, Some(h)) => Ok(Some((0, h))), // width auto-calculated
            (None, None) => Ok(None),
        }
    }
}

/// Dispatch CLI arguments.
///
/// Uses a two-pass strategy: first try parsing as `Command`, then fall back
/// to treating everything as `process` arguments (bare files default).
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // If first real arg is a known subcommand, parse normally
    let first_arg = args.get(1).map(|s| s.as_str());
    match first_arg {
        Some("process") => {
            let cmd = ProcessArgs::parse_from(&args[1..]);
            process::run(cmd)
        }
        Some("info") => {
            let cmd = InfoArgs::parse_from(&args[1..]);
            info::run(cmd)
        }
        Some("help" | "--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("zcimg {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(_) => {
            // Bare files / flags → treat as `process` args
            let cmd = ProcessArgs::parse_from(
                std::iter::once("process".to_string()).chain(args[1..].iter().cloned()),
            );
            process::run(cmd)
        }
    }
}

fn print_help() {
    eprintln!(
        "\
zcimg {} — multi-format image processor

USAGE:
    zcimg [COMMAND] [OPTIONS] <FILES>...

COMMANDS:
    process    Transcode, re-encode, or optimize images (default)
    info       Probe and display image metadata

Bare files default to `process` with balanced quality.

EXAMPLES:
    zcimg photo.jpg                          Re-encode with balanced quality
    zcimg photo.jpg -f webp -q 80 -o out.webp   Transcode to WebP
    zcimg process *.jpg -f png -o /tmp/pngs/ --report
    zcimg info photo.jpg --json

Run `zcimg process --help` or `zcimg info --help` for full options.",
        env!("CARGO_PKG_VERSION")
    );
}
