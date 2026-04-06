//! Single-image and batch processing orchestration.
//!
//! All image operations go through zenpipe::job::ImageJob.
//! This module only handles file I/O, argument mapping, and output formatting.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::args::ProcessOptions;
use crate::batch;
use crate::convert;
use crate::error::{CliError, EXIT_SUCCESS};

/// Main entry point for the process command.
pub fn run(opts: ProcessOptions) -> ExitCode {
    let is_glob = opts.input.contains('*') || opts.input.contains('?');

    if is_glob {
        return run_batch(opts);
    }

    if opts.batch_opts.srcset.is_some() {
        return run_srcset(opts);
    }

    match process_single(&opts) {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// Process a single image via ImageJob.
fn process_single(opts: &ProcessOptions) -> Result<(), CliError> {
    let start = Instant::now();

    let input_bytes = read_input(&opts.input)?;
    let converted = convert::ops_to_nodes(&opts.ops)?;
    let output_ext = output_extension(&opts.output)?;

    // Dry-run: show what would happen.
    if opts.batch_opts.dry_run {
        eprintln!(
            "dry-run: {} → {} ({} operations)",
            opts.input,
            opts.output,
            converted.nodes.len()
        );
        return Ok(());
    }

    // Explain: probe and show pipeline, don't execute.
    if opts.debug_opts.explain {
        let probe = zenpipe::job::ImageJob::new()
            .add_input_ref(0, &input_bytes)
            .probe()
            .map_err(|e| CliError::Operation(format!("{e}")))?;

        println!(
            "1. Decode {} ({}x{}, {})",
            probe.format_name(),
            probe.width,
            probe.height,
            if probe.has_alpha { "alpha" } else { "opaque" },
        );
        for (i, node) in converted.nodes.iter().enumerate() {
            let schema = node.schema();
            let params = node.to_params();
            let param_str: Vec<String> = params
                .iter()
                .filter(|(_, v)| !matches!(v, zennode::ParamValue::None))
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect();
            println!("{}. {} ({})", i + 2, schema.label, param_str.join(", "));
        }
        return Ok(());
    }

    // Build and run the job.
    let filter_converter = zenpipe::bridge::ZenFiltersConverter;
    let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&filter_converter];

    let mut job = zenpipe::job::ImageJob::new()
        .add_input(0, input_bytes)
        .add_output(1)
        .with_nodes(&converted.nodes)
        .with_converters(converters)
        .with_defaults(zenpipe::job::DefaultsPreset::Web);

    // Add secondary inputs (e.g., overlay images).
    for (io_id, path) in &converted.extra_inputs {
        let data = read_input(path)?;
        job = job.add_input(*io_id, data);
    }

    job = convert::apply_output_opts(job, &output_ext, &opts.output_opts);

    let trace_config;
    if opts.debug_opts.trace {
        trace_config = zenpipe::trace::TraceConfig::metadata_only();
        job = job.with_trace(&trace_config);
    }

    let result = job.run().map_err(|e| CliError::Operation(format!("{e}")))?;

    let encode = result
        .encode_results
        .first()
        .ok_or_else(|| CliError::Operation("no encode result produced".into()))?;

    write_output(&opts.output, &encode.bytes)?;

    // Status output.
    if opts.debug_opts.trace {
        let elapsed = start.elapsed();
        eprintln!("total:      {:.1}ms", elapsed.as_secs_f64() * 1000.0);
        eprintln!(
            "output:     {} ({:.2} bpp)",
            format_size(encode.bytes.len()),
            bits_per_pixel(encode.bytes.len(), encode.width, encode.height)
        );
    } else if let Some(d) = result.decode_infos.first() {
        let elapsed = start.elapsed();
        eprintln!(
            "{}x{} {} → {}x{} {} ({}) in {:.0}ms",
            d.width,
            d.height,
            d.format_name(),
            encode.width,
            encode.height,
            &encode.extension,
            format_size(encode.bytes.len()),
            elapsed.as_secs_f64() * 1000.0,
        );
    }

    Ok(())
}

/// Batch: process files matching a glob pattern.
fn run_batch(opts: ProcessOptions) -> ExitCode {
    let paths = match batch::expand_glob(&opts.input) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return e.exit_code();
        }
    };

    if paths.is_empty() {
        eprintln!("error: no files matched '{}'", opts.input);
        return ExitCode::from(1);
    }

    let total = paths.len();
    let succeeded = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let num_jobs = rayon::current_num_threads();
    eprintln!("processing {total} files with {num_jobs} threads...");

    rayon::scope(|s| {
        for (i, input_path) in paths.iter().enumerate() {
            let output_template = &opts.output;
            let ops = &opts.ops;
            let output_opts = &opts.output_opts;
            let debug_opts = &opts.debug_opts;
            let batch_opts = &opts.batch_opts;
            let succeeded = &succeeded;
            let failed = &failed;

            s.spawn(move |_| {
                let input_str = input_path.to_string_lossy().to_string();
                let output_path = batch::expand_template(output_template, input_path, i);

                if batch_opts.dry_run {
                    eprintln!("dry-run: {} → {}", input_str, output_path);
                    succeeded.fetch_add(1, Ordering::Relaxed);
                    return;
                }

                let single_opts = ProcessOptions {
                    input: input_str.clone(),
                    output: output_path,
                    ops: ops.clone(),
                    output_opts: output_opts.clone(),
                    batch_opts: batch_opts.clone(),
                    debug_opts: debug_opts.clone(),
                };

                match process_single(&single_opts) {
                    Ok(()) => {
                        succeeded.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("error: {input_str}: {e}");
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    let ok = succeeded.load(Ordering::Relaxed);
    let fail = failed.load(Ordering::Relaxed);
    eprintln!("batch: {ok}/{total} succeeded");

    if fail > 0 {
        let e = CliError::Partial {
            succeeded: ok,
            failed: fail,
        };
        eprintln!("{e}");
        e.exit_code()
    } else {
        ExitCode::from(EXIT_SUCCESS)
    }
}

/// Srcset: generate multiple sizes from one input.
fn run_srcset(opts: ProcessOptions) -> ExitCode {
    let widths = match &opts.batch_opts.srcset {
        Some(w) => w.clone(),
        None => return ExitCode::from(EXIT_SUCCESS),
    };

    let formats: Vec<String> = opts.batch_opts.formats.clone().unwrap_or_default();

    let input_bytes = match read_input(&opts.input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return e.exit_code();
        }
    };

    let mut tasks: Vec<(u32, String)> = Vec::new();
    if formats.is_empty() {
        let ext = match output_extension(&opts.output) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{e}");
                return e.exit_code();
            }
        };
        for &w in &widths {
            tasks.push((w, ext.clone()));
        }
    } else {
        for &w in &widths {
            for fmt in &formats {
                tasks.push((w, fmt.clone()));
            }
        }
    }

    let total = tasks.len();
    let succeeded = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let input_path = Path::new(&opts.input);

    rayon::scope(|s| {
        for (width, ext) in &tasks {
            let input_bytes = &input_bytes;
            let ops = &opts.ops;
            let output_opts = &opts.output_opts;
            let output_template = &opts.output;
            let succeeded = &succeeded;
            let failed = &failed;

            s.spawn(move |_| {
                let mut ops = ops.clone();
                ops.resize = Some(width.to_string());

                let converted = match convert::ops_to_nodes(&ops) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("error: srcset {width}w: {e}");
                        failed.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                };

                let filter_converter = zenpipe::bridge::ZenFiltersConverter;
                let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&filter_converter];

                let mut job = zenpipe::job::ImageJob::new()
                    .add_input_ref(0, input_bytes)
                    .add_output(1)
                    .with_nodes(&converted.nodes)
                    .with_converters(converters)
                    .with_defaults(zenpipe::job::DefaultsPreset::Web);

                job = convert::apply_output_opts(job, ext, output_opts);

                match job.run() {
                    Ok(res) => {
                        if let Some(encode) = res.encode_results.first() {
                            let out = expand_srcset_template(
                                output_template,
                                input_path,
                                encode.width,
                                encode.height,
                                ext,
                            );
                            if let Err(e) = write_output(&out, &encode.bytes) {
                                eprintln!("error: srcset {width}w: {e}");
                                failed.fetch_add(1, Ordering::Relaxed);
                            } else {
                                succeeded.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("error: srcset {width}w: {e}");
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    let ok = succeeded.load(Ordering::Relaxed);
    let fail = failed.load(Ordering::Relaxed);
    eprintln!("srcset: {ok}/{total} succeeded");

    if fail > 0 {
        ExitCode::from(4)
    } else {
        ExitCode::from(EXIT_SUCCESS)
    }
}

// ─── I/O helpers ───

fn read_input(path: &str) -> Result<Vec<u8>, CliError> {
    if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| CliError::Input(format!("stdin: {e}")))?;
        Ok(buf)
    } else {
        std::fs::read(path).map_err(|e| CliError::Input(format!("{path}: {e}")))
    }
}

fn write_output(path: &str, data: &[u8]) -> Result<(), CliError> {
    if path == "-" {
        std::io::stdout()
            .write_all(data)
            .map_err(|e| CliError::Output(format!("stdout: {e}")))?;
        std::io::stdout()
            .flush()
            .map_err(|e| CliError::Output(format!("stdout flush: {e}")))?;
    } else {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CliError::Output(format!("mkdir {}: {e}", parent.display())))?;
            }
        }
        std::fs::write(path, data).map_err(|e| CliError::Output(format!("{path}: {e}")))?;
    }
    Ok(())
}

fn output_extension(path: &str) -> Result<String, CliError> {
    if path == "-" {
        return Ok("jpg".into());
    }
    let clean = path.rsplit('/').next().unwrap_or(path);
    Path::new(clean)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .ok_or_else(|| {
            CliError::Input(format!(
                "cannot determine format from '{path}' — add a file extension"
            ))
        })
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn bits_per_pixel(bytes: usize, w: u32, h: u32) -> f64 {
    let pixels = w as f64 * h as f64;
    if pixels == 0.0 {
        0.0
    } else {
        (bytes as f64 * 8.0) / pixels
    }
}

fn expand_srcset_template(template: &str, input_path: &Path, w: u32, h: u32, ext: &str) -> String {
    let name = input_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    template
        .replace("{name}", &name)
        .replace("{w}", &w.to_string())
        .replace("{h}", &h.to_string())
        .replace("{ext}", ext)
}
