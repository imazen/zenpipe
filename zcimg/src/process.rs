//! Unified processing pipeline: decode → orient → resize → encode.
//!
//! Currently uses direct DecodeRequest/EncodeRequest (no resize support)
//! because the zencodecs pipeline feature is disabled due to a zenresize
//! archmage version conflict. When pipeline is re-enabled, this can delegate
//! to `Pipeline::from_bytes()` for resize operations.

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata, PixelBufferConvertTypedExt as _};

use crate::batch::{self, BatchSummary, FileResult};
use crate::output::OutputConfig;
use crate::{PresetArg, ProcessArgs};

/// Run the `process` subcommand.
pub fn run(args: ProcessArgs) -> anyhow::Result<()> {
    let files = batch::expand_inputs(&args.files)?;

    if files.is_empty() {
        anyhow::bail!("no image files found");
    }

    let target_format = args.resolve_format();
    let output_config = OutputConfig::new(
        args.output.as_deref(),
        args.in_place,
        &args.suffix,
        args.force,
        args.dry_run,
        target_format,
    )?;

    let input_count = files.len();
    let summary = Mutex::new(BatchSummary::new());

    // Check if resize is requested
    let dimensions = args.resolve_dimensions()?;
    if dimensions.is_some() {
        eprintln!(
            "warning: resize is not yet supported (zenresize integration pending). \
             Dimensions will be ignored."
        );
    }

    let jobs = args.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    if files.len() == 1 {
        // Single file — no progress bar
        let result = process_one(&files[0], &args, &output_config, input_count);
        if let Some(ref err) = result.error {
            eprintln!("error: {}: {}", files[0].display(), err);
        } else if !result.skipped && !args.dry_run {
            if let (Some(out_size), Some(out_path)) = (result.output_size, &result.output_path) {
                let change = if result.input_size > 0 {
                    let pct = (out_size as f64 - result.input_size as f64)
                        / result.input_size as f64
                        * 100.0;
                    format!(" ({:+.1}%)", pct)
                } else {
                    String::new()
                };
                eprintln!(
                    "{} -> {} ({}{change})",
                    batch::format_size(result.input_size),
                    batch::format_size(out_size),
                    out_path.display(),
                );
            }
        } else if result.skipped {
            eprintln!("skipped (output would be larger)");
        }
        summary.lock().unwrap().push(result);
    } else {
        // Batch — use rayon + progress bar
        let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;

        let pb = ProgressBar::new(files.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                .unwrap()
                .progress_chars("=>-"),
        );

        pool.install(|| {
            files.par_iter().for_each(|path| {
                let result = process_one(path, &args, &output_config, input_count);
                if let Some(ref err) = result.error {
                    pb.println(format!("error: {}: {}", path.display(), err));
                }
                summary.lock().unwrap().push(result);
                pb.inc(1);
            });
        });

        pb.finish_and_clear();
    }

    let summary = summary.into_inner().unwrap();

    if args.report {
        summary.print_report();
    }

    if let Some(ref csv_path) = args.csv {
        summary.write_csv(csv_path)?;
        eprintln!("CSV report written to {}", csv_path.display());
    }

    if summary.error_count() > 0 && !args.report {
        eprintln!(
            "{} of {} files had errors",
            summary.error_count(),
            summary.results.len()
        );
    }

    Ok(())
}

/// Process a single file, catching errors into FileResult.
fn process_one(
    input: &Path,
    args: &ProcessArgs,
    output_config: &OutputConfig,
    input_count: usize,
) -> FileResult {
    let start = Instant::now();
    let input_size = input.metadata().map(|m| m.len()).unwrap_or(0);

    match process_inner(input, args, output_config, input_count) {
        Ok((output_path, output_size, skipped)) => FileResult {
            input_path: input.to_path_buf(),
            input_size,
            output_size: Some(output_size),
            output_path: Some(output_path),
            skipped,
            error: None,
            duration: start.elapsed(),
        },
        Err(e) => FileResult {
            input_path: input.to_path_buf(),
            input_size,
            output_size: None,
            output_path: None,
            skipped: false,
            error: Some(format!("{e:#}")),
            duration: start.elapsed(),
        },
    }
}

/// Inner processing: returns (output_path, output_size, skipped).
fn process_inner(
    input: &Path,
    args: &ProcessArgs,
    output_config: &OutputConfig,
    input_count: usize,
) -> anyhow::Result<(std::path::PathBuf, u64, bool)> {
    let data = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;

    let input_size = data.len() as u64;

    // Detect source format
    let source_format = zencodec::ImageFormatRegistry::common().detect(&data)
        .ok_or_else(|| anyhow::anyhow!("unrecognized image format: {}", input.display()))?;

    // Determine target format
    let target_format = args.resolve_format().unwrap_or(source_format);

    // Decode
    let decoded = DecodeRequest::new(&data)
        .decode()
        .with_context(|| format!("decoding {}", input.display()))?;

    let info = decoded.info().clone();
    let has_alpha = decoded.has_alpha();

    // Build metadata based on strip flags
    let owned_meta = build_metadata(&info, args);
    let meta_ref = owned_meta.as_ref();

    // Build encode request
    let mut encode_req = EncodeRequest::new(target_format);

    // Quality
    if let Some(q) = args.quality {
        encode_req = encode_req.with_quality(q);
    } else if args.lossless {
        encode_req = encode_req.with_lossless(true);
    } else if let Some(preset) = args.preset {
        match preset {
            PresetArg::Lossless => {
                encode_req = encode_req.with_lossless(true);
            }
            PresetArg::NearLossless => {
                encode_req = encode_req.with_quality(preset_quality(preset, target_format));
            }
            PresetArg::High => {
                encode_req = encode_req.with_quality(preset_quality(preset, target_format));
            }
            PresetArg::Balanced => {
                encode_req = encode_req.with_quality(preset_quality(preset, target_format));
            }
            PresetArg::Small => {
                encode_req = encode_req.with_quality(preset_quality(preset, target_format));
            }
        }
    } else {
        // Default: balanced
        encode_req = encode_req.with_quality(preset_quality(PresetArg::Balanced, target_format));
    }

    if let Some(effort) = args.effort {
        encode_req = encode_req.with_effort(effort);
    }

    if let Some(ref meta) = meta_ref {
        encode_req = encode_req.with_metadata(meta);
    }

    // Encode based on pixel type
    let is_grayscale = decoded.descriptor().is_grayscale();
    let encoded = if has_alpha {
        let pixels = decoded.into_buffer().to_rgba8();
        encode_req.encode_rgba8(pixels.as_imgref())
    } else if is_grayscale {
        let pixels = decoded.into_buffer().to_gray8();
        encode_req.encode_gray8(pixels.as_imgref())
    } else {
        let pixels = decoded.into_buffer().to_rgb8();
        encode_req.encode_rgb8(pixels.as_imgref())
    }
    .with_context(|| format!("encoding {} as {:?}", input.display(), target_format))?;

    let output_size = encoded.data().len() as u64;

    // Skip-if-larger check
    if args.skip_if_larger && output_size >= input_size {
        return Ok((input.to_path_buf(), output_size, true));
    }

    // Resolve output path
    let output_path = output_config.resolve(input, input_count)?;

    // Check writable
    output_config.check_writable(input, &output_path)?;

    // Write (or dry-run)
    if output_config.dry_run {
        eprintln!(
            "dry-run: {} -> {} ({})",
            input.display(),
            output_path.display(),
            batch::format_size(output_size),
        );
    } else {
        OutputConfig::ensure_parent(&output_path)?;
        std::fs::write(&output_path, encoded.data())
            .with_context(|| format!("writing {}", output_path.display()))?;
    }

    Ok((output_path, output_size, false))
}

/// Build metadata to embed, applying strip flags.
fn build_metadata(
    info: &zencodec::ImageInfo,
    args: &ProcessArgs,
) -> Option<Metadata> {
    if args.strip_all {
        return None;
    }

    if args.preserve_icc {
        // Keep only ICC
        let mut meta = Metadata::none();
        if let Some(ref icc) = info.source_color.icc_profile {
            meta = meta.with_icc(icc.clone());
        }
        return Some(meta);
    }

    let mut meta = Metadata::none();

    if !args.strip_icc {
        if let Some(ref icc) = info.source_color.icc_profile {
            meta = meta.with_icc(icc.clone());
        }
    }
    if !args.strip_exif {
        if let Some(ref exif) = info.embedded_metadata.exif {
            meta = meta.with_exif(exif.clone());
        }
    }
    if !args.strip_xmp {
        if let Some(ref xmp) = info.embedded_metadata.xmp {
            meta = meta.with_xmp(xmp.clone());
        }
    }

    Some(meta)
}

/// Map a quality preset to a concrete quality value for a given format.
///
/// Mirrors `zencodecs::pipeline::quality::QualityPreset::for_format`.
fn preset_quality(preset: PresetArg, format: ImageFormat) -> f32 {
    match preset {
        PresetArg::Lossless => 100.0,
        PresetArg::NearLossless => match format {
            ImageFormat::Jpeg => 97.0,
            ImageFormat::WebP => 95.0,
            ImageFormat::Avif => 95.0,
            ImageFormat::Jxl => 99.5,
            _ => 97.0,
        },
        PresetArg::High => match format {
            ImageFormat::Jpeg => 90.0,
            ImageFormat::WebP => 90.0,
            ImageFormat::Avif => 85.0,
            ImageFormat::Jxl => 99.0,
            _ => 90.0,
        },
        PresetArg::Balanced => match format {
            ImageFormat::Jpeg => 80.0,
            ImageFormat::WebP => 80.0,
            ImageFormat::Avif => 70.0,
            ImageFormat::Jxl => 98.0,
            _ => 80.0,
        },
        PresetArg::Small => match format {
            ImageFormat::Jpeg => 60.0,
            ImageFormat::WebP => 60.0,
            ImageFormat::Avif => 45.0,
            ImageFormat::Jxl => 96.0,
            _ => 60.0,
        },
    }
}
