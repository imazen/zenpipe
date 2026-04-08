//! JSON job execution with io_id-based I/O and fan-in/fan-out.
//!
//! All image operations go through zenpipe::job::ImageJob.
//! The CLI only handles file I/O and argument mapping.
//!
//! ## JSON schema
//!
//! ```json
//! {
//!   "nodes": [
//!     {"zenresize.constrain": {"w": 800, "h": 600, "mode": "within"}},
//!     {"zenpipe.overlay": {"io_id": 5, "x": 10, "y": 10, "opacity": 0.7}}
//!   ],
//!   "decode_io_id": 0,
//!   "outputs": [
//!     {"io_id": 1, "quality": 85},
//!     {"io_id": 2, "quality": 70, "lossless": true}
//!   ]
//! }
//! ```

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde::Deserialize;

use crate::args::JobArgs;
use crate::error::{CliError, EXIT_SUCCESS};

// ─── JSON schema types ───

#[derive(Deserialize, Debug)]
pub struct JobDef {
    pub nodes: serde_json::Value,

    #[serde(default)]
    pub decode_io_id: i32,

    pub outputs: Vec<OutputDef>,
}

#[derive(Deserialize, Debug)]
pub struct OutputDef {
    pub io_id: i32,
    pub quality: Option<f32>,
    pub effort: Option<u32>,
    #[serde(default)]
    pub lossless: bool,
    pub metadata: Option<String>,
    pub hdr: Option<String>,
}

// ─── Execution ───

pub fn run(args: JobArgs) -> ExitCode {
    match run_inner(&args) {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

fn run_inner(args: &JobArgs) -> Result<(), CliError> {
    let start = Instant::now();

    let io_map = parse_io_mappings(&args.io_mappings)?;

    let job_json = std::fs::read_to_string(&args.job_file)
        .map_err(|e| CliError::Input(format!("{}: {e}", args.job_file)))?;
    let job_def: JobDef = serde_json::from_str(&job_json)
        .map_err(|e| CliError::Input(format!("invalid job JSON: {e}")))?;

    if !io_map.contains_key(&job_def.decode_io_id) {
        return Err(CliError::Input(format!(
            "no --io mapping for decode io_id {}",
            job_def.decode_io_id
        )));
    }
    for out in &job_def.outputs {
        if !io_map.contains_key(&out.io_id) {
            return Err(CliError::Input(format!(
                "no --io mapping for output io_id {}",
                out.io_id
            )));
        }
    }
    if job_def.outputs.is_empty() {
        return Err(CliError::Input("job has no outputs".into()));
    }

    // Parse nodes from JSON via the registry.
    let registry = zenpipe::full_registry();
    let nodes = registry
        .pipeline_from_json(&job_def.nodes)
        .map_err(|e| CliError::Operation(format!("node parsing failed: {e}")))?;

    // Dry-run: show plan without reading files.
    if args.dry_run {
        eprintln!(
            "job: {} nodes, {} outputs",
            nodes.len(),
            job_def.outputs.len()
        );
        for (&id, path) in &io_map {
            if job_def.outputs.iter().any(|o| o.io_id == id) {
                eprintln!("  output io_id {id} → {path}");
            } else {
                eprintln!("  input  io_id {id} ← {path}");
            }
        }
        return Ok(());
    }

    // Read all non-output io_ids as raw bytes. Zenpipe handles all decoding.
    let input_data = read_all_inputs(&io_map, &job_def)?;

    // Fan-out: parallel encode across outputs.
    let total = job_def.outputs.len();
    let succeeded = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);

    if total == 1 {
        let out = &job_def.outputs[0];
        let out_path = &io_map[&out.io_id];
        execute_single_output(&input_data, &nodes, &job_def, out, out_path, args.trace)?;
    } else {
        rayon::scope(|s| {
            for out in &job_def.outputs {
                let input_data = &input_data;
                let nodes = &nodes;
                let job_def = &job_def;
                let out_path = &io_map[&out.io_id];
                let succeeded = &succeeded;
                let failed = &failed;
                let trace = args.trace;

                s.spawn(move |_| {
                    match execute_single_output(input_data, nodes, job_def, out, out_path, trace) {
                        Ok(()) => {
                            succeeded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            eprintln!("error: io_id {}: {e}", out.io_id);
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });

        let ok = succeeded.load(Ordering::Relaxed);
        let fail = failed.load(Ordering::Relaxed);
        if fail > 0 {
            return Err(CliError::Partial {
                succeeded: ok,
                failed: fail,
            });
        }
    }

    let elapsed = start.elapsed();
    eprintln!(
        "job: {} outputs in {:.0}ms",
        total,
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

/// Execute the pipeline for a single output. All image work is ImageJob's.
fn execute_single_output(
    input_data: &HashMap<i32, Vec<u8>>,
    nodes: &[Box<dyn zennode::NodeInstance>],
    job_def: &JobDef,
    out: &OutputDef,
    out_path: &str,
    trace: bool,
) -> Result<(), CliError> {
    let output_ext = output_extension(out_path)?;

    let filter_converter = zenpipe::bridge::ZenFiltersConverter;
    let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&filter_converter];

    let mut job = zenpipe::job::ImageJob::new()
        .add_output(1)
        .with_nodes(nodes)
        .with_converters(converters)
        .with_decode_io(job_def.decode_io_id)
        .with_defaults(zenpipe::job::DefaultsPreset::Web)
        .with_output_extension(&output_ext);

    // Add all inputs by io_id.
    for (&io_id, data) in input_data {
        job = job.add_input_ref(io_id, data);
    }

    if let Some(q) = out.quality {
        job = job.with_quality(q);
    }
    if out.lossless {
        job = job.with_lossless(true);
    }
    if let Some(effort) = out.effort {
        job = job.with_codec_hint("effort", &effort.to_string());
    }

    job = job.with_metadata_policy(match out.metadata.as_deref() {
        Some("preserve") => zenpipe::job::MetadataPolicy::PreserveAll,
        Some("strip") => zenpipe::job::MetadataPolicy::StripAll,
        _ => zenpipe::job::MetadataPolicy::WebDefault,
    });
    job = job.with_gain_map_mode(match out.hdr.as_deref() {
        Some("strip") => zenpipe::job::GainMapMode::Discard,
        _ => zenpipe::job::GainMapMode::Preserve,
    });

    let trace_config;
    if trace {
        trace_config = zenpipe::trace::TraceConfig::metadata_only();
        job = job.with_trace(&trace_config);
    }

    let result = job.run().map_err(|e| CliError::Operation(format!("{e}")))?;

    let encode = result
        .encode_results
        .first()
        .ok_or_else(|| CliError::Operation("no encode result".into()))?;

    write_output(out_path, &encode.bytes)?;

    eprintln!(
        "  io_id {} → {} ({}x{}, {})",
        out.io_id,
        out_path,
        encode.width,
        encode.height,
        format_size(encode.bytes.len()),
    );

    Ok(())
}

// ─── I/O helpers ───

fn read_all_inputs(
    io_map: &HashMap<i32, String>,
    job_def: &JobDef,
) -> Result<HashMap<i32, Vec<u8>>, CliError> {
    let output_ids: Vec<i32> = job_def.outputs.iter().map(|o| o.io_id).collect();
    let mut data = HashMap::new();
    for (&id, path) in io_map {
        if output_ids.contains(&id) {
            continue;
        }
        let bytes = if path == "-" {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| CliError::Input(format!("stdin: {e}")))?;
            buf
        } else {
            std::fs::read(path).map_err(|e| CliError::Input(format!("{path}: {e}")))?
        };
        data.insert(id, bytes);
    }
    Ok(data)
}

fn parse_io_mappings(mappings: &[String]) -> Result<HashMap<i32, String>, CliError> {
    let mut map = HashMap::new();
    for m in mappings {
        let (id_str, path) = m.split_once('=').ok_or_else(|| {
            CliError::Input(format!("invalid --io format '{m}', expected ID=PATH"))
        })?;
        let id: i32 = id_str
            .parse()
            .map_err(|_| CliError::Input(format!("invalid io_id '{id_str}' in --io")))?;
        map.insert(id, path.to_string());
    }
    Ok(map)
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
