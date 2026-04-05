//! CLI argument parsing.
//!
//! Syntax: `zenpipe <input> [operations...] <output>`
//! Subcommands: `zenpipe info <files...>`, `zenpipe compare <a> <b>`

use clap::Parser;

// ─── Top-level CLI ───

#[derive(Parser, Debug)]
#[command(
    name = "zenpipe",
    about = "Fast, safe image processing — one binary to replace ImageMagick, vips, and Pillow.",
    version,
    after_help = "Format is inferred from the output file extension. Stdin/stdout: use '-'."
)]
pub struct Cli {
    #[command(subcommand)]
    pub sub: Option<Subcommand>,

    // When no subcommand, positional args are input(s) and output.
    /// Input file path or glob pattern (use '-' for stdin).
    #[arg(value_name = "INPUT")]
    pub input: Option<String>,

    /// Output file path or template (use '-' for stdout).
    #[arg(value_name = "OUTPUT")]
    pub output: Option<String>,

    #[command(flatten)]
    pub ops: Operations,

    #[command(flatten)]
    pub output_opts: OutputOptions,

    #[command(flatten)]
    pub batch_opts: BatchOptions,

    #[command(flatten)]
    pub debug_opts: DebugOptions,
}

#[derive(clap::Subcommand, Debug)]
pub enum Subcommand {
    /// Execute a JSON job definition (fan-in/fan-out, io_id-based I/O).
    Job(JobArgs),
    /// Show image metadata (dimensions, format, color space, EXIF).
    Info(InfoArgs),
    /// Compare two images (perceptual difference metrics).
    Compare(CompareArgs),
}

// ─── Job subcommand ───

#[derive(Parser, Debug)]
pub struct JobArgs {
    /// Path to JSON job file.
    pub job_file: String,

    /// Map io_id to file path. Repeatable: --io 0=input.jpg --io 1=out.webp
    #[arg(long = "io", value_name = "ID=PATH")]
    pub io_mappings: Vec<String>,

    /// Number of parallel encoding threads for fan-out.
    #[arg(long, short = 'j')]
    pub jobs: Option<usize>,

    /// Show what would be done without processing.
    #[arg(long)]
    pub dry_run: bool,

    /// Detailed timing per pipeline step.
    #[arg(long)]
    pub trace: bool,
}

// ─── Info subcommand ───

#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// Image file(s) or glob pattern.
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// ─── Compare subcommand ───

#[derive(Parser, Debug)]
pub struct CompareArgs {
    /// First image.
    pub a: String,
    /// Second image.
    pub b: String,
}

// ─── Operations (chained left to right) ───

#[derive(Parser, Debug, Clone, Default)]
pub struct Operations {
    // ── Geometry ──
    /// Resize: WxH, W, W!, WxH^, WxH#, or N%.
    /// Suffixes: ! = exact/distort, ^ = fill/crop, # = pad/letterbox.
    #[arg(long)]
    pub resize: Option<String>,

    /// Resampling filter for resize (default: robidoux).
    #[arg(long, default_value = None)]
    pub filter: Option<String>,

    /// Crop: x,y,w,h in pixels or percentages (e.g. 10%,10%,80%,80%).
    /// Use "auto" for automatic whitespace crop.
    #[arg(long)]
    pub crop: Option<String>,

    /// Rotate: 90, 180, 270 for cardinal, or any angle for arbitrary.
    /// Use "auto" for auto-deskew.
    #[arg(long)]
    pub rotate: Option<String>,

    /// Flip: h (horizontal), v (vertical).
    #[arg(long)]
    pub flip: Option<String>,

    /// EXIF orientation: "auto" to apply and strip, or 1-8 to force.
    #[arg(long)]
    pub orient: Option<String>,

    /// Padding in pixels: single value or top,right,bottom,left.
    #[arg(long)]
    pub pad: Option<String>,

    /// Background color for padding (default: white). Hex or name.
    #[arg(long)]
    pub bg: Option<String>,

    // ── Filters ──
    /// Exposure adjustment in stops (e.g. 1.5 = 2x brighter).
    #[arg(long, allow_hyphen_values = true)]
    pub exposure: Option<f32>,

    /// Contrast adjustment (-1..1).
    #[arg(long, allow_hyphen_values = true)]
    pub contrast: Option<f32>,

    /// Brightness adjustment (-100..100, linear like CSS).
    #[arg(long, allow_hyphen_values = true)]
    pub brightness: Option<f32>,

    /// Saturation factor (1.0 = unchanged).
    #[arg(long)]
    pub saturation: Option<f32>,

    /// Vibrance (0..1).
    #[arg(long)]
    pub vibrance: Option<f32>,

    /// Warm/cool color temperature shift.
    #[arg(long, allow_hyphen_values = true)]
    pub temperature: Option<f32>,

    /// Green/magenta tint shift.
    #[arg(long, allow_hyphen_values = true)]
    pub tint: Option<f32>,

    /// Local contrast enhancement (0..1).
    #[arg(long)]
    pub clarity: Option<f32>,

    /// Unsharp mask: amount or amount,sigma.
    #[arg(long)]
    pub sharpen: Option<String>,

    /// Gaussian blur sigma.
    #[arg(long)]
    pub blur: Option<f32>,

    /// Luminance noise reduction (0..1).
    #[arg(long)]
    pub denoise: Option<f32>,

    /// Highlight recovery (-1..1).
    #[arg(long, allow_hyphen_values = true)]
    pub highlights: Option<f32>,

    /// Shadow lift (-1..1).
    #[arg(long, allow_hyphen_values = true)]
    pub shadows: Option<f32>,

    /// Black point crush (0..1).
    #[arg(long)]
    pub black_point: Option<f32>,

    /// White point clip (0..1).
    #[arg(long)]
    pub white_point: Option<f32>,

    /// Vignette strength (0..1).
    #[arg(long)]
    pub vignette: Option<f32>,

    /// Dehazing strength (0..1).
    #[arg(long)]
    pub dehaze: Option<f32>,

    /// Film grain amount (0..1).
    #[arg(long)]
    pub grain: Option<f32>,

    /// Auto exposure + levels + clarity + vibrance.
    #[arg(long)]
    pub auto_enhance: bool,

    /// Automatic histogram stretch.
    #[arg(long)]
    pub auto_levels: bool,

    /// Scene-adaptive exposure correction.
    #[arg(long)]
    pub auto_exposure: bool,

    /// Automatic white balance.
    #[arg(long)]
    pub auto_wb: bool,

    // ── Color ──
    /// Convert to luminance-weighted grayscale.
    #[arg(long)]
    pub grayscale: bool,

    /// Sepia toning (0..1).
    #[arg(long)]
    pub sepia: Option<f32>,

    /// Color inversion.
    #[arg(long)]
    pub invert: bool,

    // ── Compositing ──
    /// Overlay image: path,x,y or path,anchor[,opacity].
    #[arg(long)]
    pub overlay: Option<String>,

    /// Smart watermark: path (auto-position, auto-opacity).
    #[arg(long)]
    pub watermark: Option<String>,

    // ── Querystring ──
    /// Apply an RIAPI/imageflow querystring.
    #[arg(long)]
    pub qs: Option<String>,
}

// ─── Output control ───

#[derive(Parser, Debug, Clone, Default)]
pub struct OutputOptions {
    /// Universal quality (0-100). Mapped to codec-native quality.
    #[arg(long, short)]
    pub quality: Option<f32>,

    /// Compression effort (0-10, speed vs size).
    #[arg(long)]
    pub effort: Option<u32>,

    /// Lossless mode (WebP, JXL, AVIF, PNG).
    #[arg(long)]
    pub lossless: bool,

    /// Near-lossless quality (WebP, JXL).
    #[arg(long)]
    pub near_lossless: Option<f32>,

    // ── Codec-specific ──
    /// JPEG chroma subsampling: 444, 422, 420.
    #[arg(long)]
    pub jpeg_subsampling: Option<String>,

    /// Progressive JPEG.
    #[arg(long)]
    pub jpeg_progressive: bool,

    /// 16-bit PNG output.
    #[arg(long)]
    pub png_depth: Option<u32>,

    /// JXL butteraugli distance (overrides --quality).
    #[arg(long)]
    pub jxl_distance: Option<f32>,

    /// AVIF encoder speed (1-10).
    #[arg(long)]
    pub avif_speed: Option<u32>,

    /// GIF dithering strength (0..1).
    #[arg(long)]
    pub gif_dither: Option<f32>,

    // ── Metadata ──
    /// Strip metadata: "all", "exif", "icc", or omit for strip-all.
    #[arg(long, value_name = "WHAT")]
    pub strip: Option<Option<String>>,

    /// Keep only specified metadata (comma-separated: exif,icc).
    #[arg(long)]
    pub keep: Option<String>,

    /// Preserve all metadata (default).
    #[arg(long)]
    pub preserve: bool,

    // ── HDR / Gain map ──
    /// HDR handling: preserve (default), strip, tonemap, reconstruct.
    #[arg(long)]
    pub hdr: Option<String>,
}

// ─── Batch options ───

#[derive(Parser, Debug, Clone, Default)]
pub struct BatchOptions {
    /// Number of parallel processing threads.
    #[arg(long, short = 'j')]
    pub jobs: Option<usize>,

    /// Show what would be done without processing.
    #[arg(long)]
    pub dry_run: bool,

    /// Generate responsive image set at these widths (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub srcset: Option<Vec<u32>>,

    /// Output formats for srcset (comma-separated: webp,avif,jxl).
    #[arg(long, value_delimiter = ',')]
    pub formats: Option<Vec<String>>,
}

// ─── Debug options ───

#[derive(Parser, Debug, Clone, Default)]
pub struct DebugOptions {
    /// Show the pipeline graph (what zenpipe will do).
    #[arg(long)]
    pub explain: bool,

    /// Detailed timing per pipeline step.
    #[arg(long)]
    pub trace: bool,

    /// Export pipeline as RIAPI querystring.
    #[arg(long)]
    pub print_qs: bool,
}

// ─── Resolved command ───

/// The resolved command after parsing.
pub enum Command {
    Process(Box<ProcessOptions>),
    Job(Box<JobArgs>),
    Info(InfoArgs),
    Compare(CompareArgs),
}

/// Fully resolved options for processing.
pub struct ProcessOptions {
    pub input: String,
    pub output: String,
    pub ops: Operations,
    pub output_opts: OutputOptions,
    pub batch_opts: BatchOptions,
    pub debug_opts: DebugOptions,
}

/// Parse CLI arguments into a resolved Command.
pub fn parse() -> ResolvedCli {
    let cli = Cli::parse();

    let command = if let Some(sub) = cli.sub {
        match sub {
            Subcommand::Job(args) => Command::Job(Box::new(args)),
            Subcommand::Info(args) => Command::Info(args),
            Subcommand::Compare(args) => Command::Compare(args),
        }
    } else {
        let input = cli.input.unwrap_or_else(|| {
            eprintln!("error: missing input file");
            eprintln!("usage: zenpipe <input> [operations...] <output>");
            std::process::exit(1);
        });
        let output = cli.output.unwrap_or_else(|| {
            eprintln!("error: missing output file");
            eprintln!("usage: zenpipe <input> [operations...] <output>");
            std::process::exit(1);
        });
        Command::Process(Box::new(ProcessOptions {
            input,
            output,
            ops: cli.ops,
            output_opts: cli.output_opts,
            batch_opts: cli.batch_opts,
            debug_opts: cli.debug_opts,
        }))
    };

    ResolvedCli { command }
}

pub struct ResolvedCli {
    pub command: Command,
}
