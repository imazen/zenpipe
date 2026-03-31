#!/usr/bin/env -S cargo +nightly -Zscript
//! Export zennode schemas to JSON files for documentation generation.
//!
//! Run: cargo test --features "json-schema" --lib -- schema_export::tests::dump_schemas_to_files
//!
//! This is a helper script — the actual export is done in schema_export.rs tests.

fn main() {
    eprintln!("Run this via cargo test instead:");
    eprintln!("  cargo test --features 'json-schema' --lib -- schema_export::tests::dump_schemas_to_files");
}
