#![forbid(unsafe_code)]

//! zenpipe CLI — replace ImageMagick, libvips, and Pillow with one binary.
//!
//! See CLI-SPEC.md for the full specification.

mod args;
mod batch;
mod convert;
mod error;
mod info;
mod job_json;
mod process;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Initialize rayon thread pool globally. All parallel work (fan-out,
    // batch, srcset) shares this pool. Default: one thread per logical core.
    rayon::ThreadPoolBuilder::new()
        .build_global()
        .expect("failed to initialize rayon thread pool");

    let cli = args::parse();
    match cli.command {
        args::Command::Process(opts) => process::run(*opts),
        args::Command::Job(opts) => job_json::run(*opts),
        args::Command::Info(opts) => info::run_info(opts),
        args::Command::Compare(opts) => info::run_compare(opts),
    }
}
