//! `zencodecs` — a minimal, capable image transcoding CLI over the zencodecs
//! library: MxN any→any, lossless, and minimally-lossless (zensim IQA).
//!
//! Deliberately thin — all codec work lives in the library; this is argument
//! parsing + file IO. Built so batch jobs (e.g. the imazen-26 corpus
//! conversion) can be a `find` + per-file invocation instead of bespoke Rust.
//! Tracking: imazen/zenpipe#68.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use zencodecs::{
    transcode, transcode_to_quality, AllowedFormats, FormatDecision, ImageFormat, QualityIntent,
    QualityTarget, TranscodeOptions,
};

#[derive(Parser)]
#[command(
    name = "zencodecs",
    version,
    about = "Minimal, capable image transcoder (MxN, lossless / minimally-lossless)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Transcode one image to another format.
    Convert(ConvertArgs),
    /// Probe an image: detected format + dimensions + supplements, as JSON.
    Probe { input: PathBuf },
}

#[derive(Args)]
struct ConvertArgs {
    /// Source image (any decodable format).
    input: PathBuf,
    /// Destination (format inferred from its extension unless `--format` is given).
    output: PathBuf,
    /// Output format (png|jpeg|webp|avif|jxl|gif|bmp); overrides the extension.
    #[arg(long)]
    format: Option<String>,
    /// Lossy quality 0–100 (codec-calibrated). Ignored with --lossless/--target-quality.
    #[arg(long)]
    quality: Option<f32>,
    /// Encode losslessly.
    #[arg(long, conflicts_with_all = ["quality", "target_quality"])]
    lossless: bool,
    /// Minimally-lossless: smallest size meeting this zensim-A score (0–100) vs the original.
    #[arg(long, conflicts_with = "quality")]
    target_quality: Option<f32>,
    /// Quiet: suppress the per-file summary on stderr.
    #[arg(short, long)]
    quiet: bool,
}

/// Map a format name or extension to an [`ImageFormat`] the encoder supports.
fn parse_format(s: &str) -> Option<ImageFormat> {
    Some(
        match s
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "webp" => ImageFormat::WebP,
            "avif" => ImageFormat::Avif,
            "jxl" => ImageFormat::Jxl,
            "gif" => ImageFormat::Gif,
            "bmp" => ImageFormat::Bmp,
            _ => return None,
        },
    )
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zencodecs: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Convert(a) => convert(a),
        Cmd::Probe { input } => probe_cmd(&input),
    }
}

fn convert(a: ConvertArgs) -> Result<(), String> {
    let data = std::fs::read(&a.input).map_err(|e| format!("read {}: {e}", a.input.display()))?;
    let fmt = match &a.format {
        Some(f) => parse_format(f).ok_or_else(|| format!("unknown --format '{f}'"))?,
        None => a
            .output
            .extension()
            .and_then(|e| e.to_str())
            .and_then(parse_format)
            .ok_or_else(|| {
                format!(
                    "can't infer output format from '{}'; pass --format",
                    a.output.display()
                )
            })?,
    };

    let registry = AllowedFormats::all();
    let opts = TranscodeOptions::default();
    let out = if let Some(tq) = a.target_quality {
        // Minimally-lossless: smallest byte size meeting the zensim-A target.
        transcode_to_quality(&data, fmt, QualityTarget::Absolute(tq), &opts, &registry)
            .map_err(|e| format!("transcode: {e}"))?
    } else {
        let mut decision = FormatDecision::for_format(fmt);
        if a.lossless {
            decision.lossless = true;
        } else if let Some(q) = a.quality {
            decision.quality = QualityIntent::from_quality(q);
        }
        transcode(&data, &decision, &opts, &registry).map_err(|e| format!("transcode: {e}"))?
    };

    std::fs::write(&a.output, &out.data)
        .map_err(|e| format!("write {}: {e}", a.output.display()))?;
    if !a.quiet {
        eprintln!(
            "{} -> {} ({:?}, {} KiB)",
            a.input.display(),
            a.output.display(),
            out.format,
            out.data.len() / 1024
        );
    }
    Ok(())
}

fn probe_cmd(input: &Path) -> Result<(), String> {
    let data = std::fs::read(input).map_err(|e| format!("read {}: {e}", input.display()))?;
    let info =
        zencodecs::probe(&data, &AllowedFormats::all()).map_err(|e| format!("probe: {e}"))?;
    println!(
        "{{\"format\":\"{:?}\",\"width\":{},\"height\":{},\"gain_map\":{},\"depth_map\":{}}}",
        info.format, info.width, info.height, info.supplements.gain_map, info.supplements.depth_map
    );
    Ok(())
}
