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
    transcode, transcode_to_quality, AllowedFormats, FormatDecision, ImageFormat, MetadataPolicy,
    OrientationHint, QualityIntent, QualityTarget, TranscodeOptions,
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
    /// Metadata retention: exact (verbatim) | preserve | web (strip GPS/camera/
    /// timestamps, keep orientation+color) | color (color+rotation only). Default: exact.
    #[arg(long)]
    metadata: Option<String>,
    /// Matte color "R,G,B" for alpha→opaque (e.g. RGBA→JPEG). Default white.
    #[arg(long)]
    matte: Option<String>,
    /// Reconstruct the HDR rendition (gain-map HEIC / Ultra-HDR JPEG only) to a
    /// BT.2100 PQ PNG with cICP+cLLI, instead of the SDR base. Output is PNG.
    #[arg(long, conflicts_with_all = ["quality", "lossless", "target_quality", "format"])]
    hdr: bool,
    /// Keep the source EXIF orientation tag instead of baking it into the pixels
    /// (default: auto-orient, i.e. bake — display-ready, correct for PNG output).
    #[arg(long)]
    keep_orientation: bool,
    /// Quiet: suppress the per-file summary on stderr.
    #[arg(short, long)]
    quiet: bool,
}

fn parse_metadata_policy(s: &str) -> Option<MetadataPolicy> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "exact" | "preserve-exact" => MetadataPolicy::PreserveExact,
        "preserve" => MetadataPolicy::Preserve,
        "web" => MetadataPolicy::Web,
        "color" | "color-and-rotation" => MetadataPolicy::ColorAndRotation,
        _ => return None,
    })
}

fn parse_matte(s: &str) -> Option<[u8; 3]> {
    let mut it = s.split(',').map(|c| c.trim().parse::<u8>().ok());
    let rgb = [it.next()??, it.next()??, it.next()??];
    if it.next().is_some() {
        return None; // more than 3 components
    }
    Some(rgb)
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

    // HDR rendition: reconstruct the gain-map source to a PQ PNG. One file in,
    // one file out — a bash script pairs this with a plain `convert` for SDR.
    if a.hdr {
        let registry = AllowedFormats::all();
        let png = zencodecs::transcode_to_hdr_pq_png(&data, &registry, None)
            .map_err(|e| format!("hdr reconstruct: {e}"))?
            .ok_or_else(|| format!("{}: no gain map to reconstruct", a.input.display()))?;
        std::fs::write(&a.output, &png)
            .map_err(|e| format!("write {}: {e}", a.output.display()))?;
        if !a.quiet {
            eprintln!(
                "{} -> {} (PNG, BT.2100 PQ HDR, {} KiB)",
                a.input.display(),
                a.output.display(),
                png.len() / 1024
            );
        }
        return Ok(());
    }

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
    let mut opts = TranscodeOptions::default();
    // Auto-orient by default: bake EXIF orientation into the pixels so output is
    // display-ready (and correct for tag-less targets like PNG). --keep-orientation
    // leaves the tag authoritative instead.
    if !a.keep_orientation {
        opts.orientation = OrientationHint::Correct;
    }
    if let Some(m) = &a.metadata {
        opts.metadata_policy =
            parse_metadata_policy(m).ok_or_else(|| format!("unknown --metadata '{m}'"))?;
    }
    if let Some(m) = &a.matte {
        opts.matte = Some(parse_matte(m).ok_or_else(|| format!("bad --matte '{m}' (want R,G,B)"))?);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parses_names_and_extensions() {
        assert!(matches!(parse_format("png"), Some(ImageFormat::Png)));
        assert!(matches!(parse_format(".JPG"), Some(ImageFormat::Jpeg)));
        assert!(matches!(parse_format("jpeg"), Some(ImageFormat::Jpeg)));
        assert!(matches!(parse_format("WebP"), Some(ImageFormat::WebP)));
        assert!(matches!(parse_format("avif"), Some(ImageFormat::Avif)));
        assert!(matches!(parse_format("jxl"), Some(ImageFormat::Jxl)));
        assert!(parse_format("heic").is_none()); // decode tracked in #68, not an encode target
        assert!(parse_format("tiff").is_none());
    }

    #[test]
    fn matte_parses_rgb_triples() {
        assert_eq!(parse_matte("0,0,0"), Some([0, 0, 0]));
        assert_eq!(parse_matte(" 255, 128 ,0 "), Some([255, 128, 0]));
        assert!(parse_matte("1,2").is_none()); // too few
        assert!(parse_matte("1,2,3,4").is_none()); // too many
        assert!(parse_matte("1,2,300").is_none()); // out of u8 range
    }

    #[test]
    fn metadata_policy_parses_keywords() {
        assert!(matches!(
            parse_metadata_policy("exact"),
            Some(MetadataPolicy::PreserveExact)
        ));
        assert!(matches!(
            parse_metadata_policy("web"),
            Some(MetadataPolicy::Web)
        ));
        assert!(matches!(
            parse_metadata_policy("color"),
            Some(MetadataPolicy::ColorAndRotation)
        ));
        assert!(parse_metadata_policy("bogus").is_none());
    }
}
