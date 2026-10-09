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
    transcode, transcode_to_quality, AllowedFormats, EncodeSpeed, FormatDecision, ImageFormat,
    MetadataPolicy, OrientationHint, QualityIntent, QualityTarget, TranscodeOptions,
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
    /// List every structural part of each file (segments, chunks, boxes, item
    /// extents, trailers, gaps) and what the decoder does with it.
    Inventory(InventoryArgs),
}

#[derive(Args)]
struct InventoryArgs {
    /// Files, or directories to walk recursively.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Tab-separated output, one row per part, for corpus audits.
    #[arg(long)]
    tsv: bool,
    /// Only parts the decoder does not consume (plus a status row per file
    /// that has none, or no inventory).
    #[arg(long)]
    unconsumed: bool,
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
    /// Minimally-lossless by size: encode lossless AND lossy (at --quality or the
    /// codec default); keep the lossless output when it is at most FACTOR times
    /// the lossy size (default 1.5), else fall back to the lossy encode.
    #[arg(long, value_name = "FACTOR", num_args = 0..=1, default_missing_value = "1.5",
          conflicts_with_all = ["lossless", "target_quality"])]
    lossless_if_cheaper: Option<f32>,
    /// Speed preset: fastest | realtime | offline | offline-max. Maps to a
    /// per-codec effort (see zencodecs::EncodeSpeed); fastest is single-threaded.
    #[arg(long)]
    speed: Option<String>,
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
        Cmd::Inventory(a) => inventory_cmd(&a),
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
    let speed = match &a.speed {
        Some(s) => Some(EncodeSpeed::from_name(s).ok_or_else(|| {
            format!("unknown --speed '{s}' (fastest|realtime|offline|offline-max)")
        })?),
        None => None,
    };
    let decision_for = |lossless: bool| {
        let mut decision = FormatDecision::for_format(fmt);
        if lossless {
            decision.lossless = true;
        } else if let Some(q) = a.quality {
            decision.quality = QualityIntent::from_quality(q);
        }
        if let Some(speed) = speed {
            decision.quality = decision.quality.with_effort(speed.generic_effort(fmt));
        }
        decision
    };
    let out = if let Some(tq) = a.target_quality {
        // Minimally-lossless: smallest byte size meeting the zensim-A target.
        transcode_to_quality(&data, fmt, QualityTarget::Absolute(tq), &opts, &registry)
            .map_err(|e| format!("transcode: {e}"))?
    } else if let Some(factor) = a.lossless_if_cheaper {
        // Minimally-lossless by size: keep lossless when it costs at most
        // `factor`× the lossy encode, else take the lossy bytes.
        if !(factor.is_finite() && factor > 0.0) {
            return Err(format!(
                "--lossless-if-cheaper wants a positive factor, got {factor}"
            ));
        }
        if !fmt.supports_lossless() {
            return Err(format!(
                "{fmt:?} has no lossless mode for --lossless-if-cheaper"
            ));
        }
        let lossless = transcode(&data, &decision_for(true), &opts, &registry)
            .map_err(|e| format!("transcode (lossless): {e}"))?;
        let lossy = transcode(&data, &decision_for(false), &opts, &registry)
            .map_err(|e| format!("transcode (lossy): {e}"))?;
        let keep_lossless = lossless.data.len() as f64 <= factor as f64 * lossy.data.len() as f64;
        if !a.quiet {
            eprintln!(
                "lossless {} B vs lossy {} B (factor {factor}): keeping {}",
                lossless.data.len(),
                lossy.data.len(),
                if keep_lossless { "lossless" } else { "lossy" }
            );
        }
        if keep_lossless {
            lossless
        } else {
            lossy
        }
    } else {
        transcode(&data, &decision_for(a.lossless), &opts, &registry)
            .map_err(|e| format!("transcode: {e}"))?
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
    // Bug #25: `{:?}` (Debug) on `ImageFormat` is not a stable string, and for
    // `ImageFormat::Custom(&ImageFormatDefinition)` its Debug output embeds
    // unescaped double quotes (`ImageFormatDefinition { name: "pdf", .. }`),
    // which breaks this hand-built JSON literal outright. Use the format's own
    // stable, lowercase `name` (from its `ImageFormatDefinition`) instead --
    // the same identifier for both named variants ("jpeg", "png", ...) and
    // Custom ones ("dng", "raw", "pdf", ...).
    let format_name = info.format.definition().map_or("unknown", |d| d.name);
    println!(
        "{{\"format\":\"{}\",\"width\":{},\"height\":{},\"gain_map\":{},\"depth_map\":{}}}",
        format_name, info.width, info.height, info.supplements.gain_map, info.supplements.depth_map
    );
    Ok(())
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for entry in entries {
            collect_files(&entry, out)?;
        }
    } else if meta.is_file() {
        out.push(path.to_path_buf());
    }
    Ok(())
}

/// One TSV field: tabs and line breaks become spaces.
fn tsv_field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn inventory_cmd(a: &InventoryArgs) -> Result<(), String> {
    use zencodecs::inventory::{Inventory, PartId};
    let mut files = Vec::new();
    for input in &a.inputs {
        collect_files(input, &mut files)?;
    }
    if a.tsv {
        println!("file\toffset\tlength\tdepth\tkind\ttag\tdisposition\tlabel\tdetail");
    }
    fn walk(inv: &Inventory, parent: Option<PartId>, depth: usize, out: &mut Vec<(PartId, usize)>) {
        for id in inv.children(parent) {
            out.push((id, depth));
            walk(inv, Some(id), depth + 1, out);
        }
    }
    for file in &files {
        let name = file.display().to_string();
        let status = |what: &str, detail: &str| {
            if a.tsv {
                println!(
                    "{}\t-\t-\t-\tfile\t-\t{what}\t-\t{}",
                    tsv_field(&name),
                    tsv_field(detail)
                );
            } else {
                println!("{name}: {what} {detail}");
            }
        };
        let data = match std::fs::read(file) {
            Ok(d) => d,
            Err(e) => {
                status("error", &e.to_string());
                continue;
            }
        };
        let inv = match zencodecs::inventory::inventory(&data, &AllowedFormats::all()) {
            Ok(Some(inv)) => inv,
            Ok(None) => {
                status("unsupported", "this format's decoder has no inventory yet");
                continue;
            }
            Err(e) => {
                status("error", &e.to_string());
                continue;
            }
        };
        if let Err(e) = inv.validate() {
            status("invalid-inventory", &e.to_string());
        }
        let mut order = Vec::new();
        walk(&inv, None, 0, &mut order);
        let shown: Vec<_> = order
            .into_iter()
            .filter(|(id, _)| {
                !a.unconsumed || inv.get(*id).is_some_and(|p| !p.disposition.is_consumed())
            })
            .collect();
        if a.unconsumed && shown.is_empty() {
            status("clean", "every byte is consumed");
            continue;
        }
        if !a.tsv {
            if a.unconsumed {
                println!("{name}:");
            } else {
                println!("{name}:\n{inv}");
                continue;
            }
        }
        for (id, depth) in shown {
            let Some(p) = inv.get(id) else { continue };
            let label = p.label.as_deref().unwrap_or("-");
            let detail = p.detail.as_deref().unwrap_or("-");
            if a.tsv {
                println!(
                    "{}\t{}\t{}\t{depth}\t{}\t{}\t{}\t{}\t{}",
                    tsv_field(&name),
                    p.range.start,
                    p.len(),
                    p.kind.name(),
                    tsv_field(&p.tag.to_string()),
                    p.disposition,
                    tsv_field(label),
                    tsv_field(detail)
                );
            } else {
                println!(
                    "  {:>10} {:>10}  {:width$}{} {} {} {label:?} ({detail})",
                    p.range.start,
                    p.len(),
                    "",
                    p.kind.name(),
                    p.tag,
                    p.disposition,
                    width = depth * 2
                );
            }
        }
    }
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
        assert!(parse_format("heic").is_none()); // decode-only (heic-decode); not an encode target
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
