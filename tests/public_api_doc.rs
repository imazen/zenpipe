//! Regenerates the committed public-API surface snapshots
//! (`docs/public-api/<crate>.txt`, one per published/library crate) on every
//! `cargo test` run, so API changes always show up as a git diff next to the
//! code change that caused them, and the surface size stays one glance away.
//!
//! Modes (`ZEN_API_DOC` env var — set per-job in CI workflows, see ci.yml):
//! - unset / `regen` → regenerate the files in place (local default; commit the diff)
//! - `check`         → regenerate to memory, FAIL if a committed file is stale
//! - `off`           → skipped (CI matrix jobs without nightly rustdoc / the tool)
//!
//! Crates with an extra-features section use "all features except `_*`" —
//! every feature from the crate's manifest except underscore-prefixed ones,
//! which are internal/research gates and not public surface. The list is
//! computed from `cargo metadata`, so new features appear automatically.
//! zenpipe and zencodecs override that (see `CRATES`).
//!
//! Requires `cargo-public-api` (0.52+) and a nightly toolchain for rustdoc
//! JSON: `cargo install cargo-public-api --locked && rustup toolchain install nightly`

use std::path::PathBuf;
use std::process::Command;

/// How the extra (non-default) snapshot section is built for a crate.
enum ExtraSection {
    /// No extra section — the crate snapshots default features only.
    None,
    /// Dynamic: all manifest features except `default` and `_*`-prefixed.
    PublicFeatures,
    /// Pinned feature combo: (section label, `--features` csv).
    Pinned(&'static str, &'static str),
}

/// Library crates in this workspace (zenpipe-cmd is a binary; zeneditor is
/// `publish = false`). zenpipe `--all-features` and zencodecs `--features all`
/// don't build (stub codec features / gainmap drift), so zenpipe snapshots
/// default-only and zencodecs uses the documented-good `jxl-encode,cms` combo.
const CRATES: &[(&str, ExtraSection)] = &[
    ("zenpipe", ExtraSection::None),
    (
        "zencodecs",
        ExtraSection::Pinned("jxl-encode,cms", "jxl-encode,cms"),
    ),
    ("zenfilters", ExtraSection::PublicFeatures),
    ("zenlayout", ExtraSection::PublicFeatures),
];

fn run(args: &[&str]) -> Vec<u8> {
    let out = Command::new("cargo")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to run cargo {} ({e}); for public-api: install with \
                 `cargo install cargo-public-api --locked` and ensure a nightly \
                 toolchain exists (`rustup toolchain install nightly`), or set \
                 ZEN_API_DOC=off to skip this test",
                args[0]
            )
        });
    assert!(
        out.status.success(),
        "cargo {} failed (set ZEN_API_DOC=off to skip):\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn surface(package: &str, feature_args: &[&str]) -> Vec<String> {
    let mut args = vec!["public-api", "-p", package, "--simplified"];
    args.extend_from_slice(feature_args);
    String::from_utf8(run(&args))
        .expect("cargo public-api emitted non-UTF8")
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.is_empty())
        .collect()
}

/// All manifest features of `package` except `default` and underscore-
/// prefixed internal gates, sorted for determinism.
fn public_features(package: &str) -> Vec<String> {
    let meta: serde_json::Value =
        serde_json::from_slice(&run(&["metadata", "--no-deps", "--format-version", "1"]))
            .expect("cargo metadata JSON");
    let pkg = meta["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"] == package)
        .unwrap_or_else(|| panic!("{package} not in workspace metadata"));
    let mut feats: Vec<String> = pkg["features"]
        .as_object()
        .expect("features map")
        .keys()
        .filter(|k| *k != "default" && !k.starts_with('_'))
        .cloned()
        .collect();
    feats.sort();
    feats
}

#[test]
fn public_api_surface_docs_are_current() {
    match std::env::var("ZEN_API_DOC").as_deref() {
        Ok("off") => {
            eprintln!("ZEN_API_DOC=off — public-API snapshot regen skipped by caller");
            return;
        }
        // This repo's CI rides imazen/zen-workspace's reusable rust-ci.yml,
        // which has no env passthrough — its matrix jobs can't set
        // ZEN_API_DOC=off the way self-contained workflows do. Treat "unset
        // under GitHub Actions" as off; the dedicated api-doc job in ci.yml
        // sets ZEN_API_DOC=check explicitly.
        Err(_) if std::env::var_os("GITHUB_ACTIONS").is_some() => {
            eprintln!(
                "ZEN_API_DOC unset under GITHUB_ACTIONS — snapshot regen skipped \
                 (the api-doc job runs the ZEN_API_DOC=check gate)"
            );
            return;
        }
        Ok("check") | Ok("regen") | Err(_) => {}
        Ok(other) => panic!("unknown ZEN_API_DOC value {other:?} (off|check|regen)"),
    }
    let check = std::env::var("ZEN_API_DOC").as_deref() == Ok("check");

    for (package, extra) in CRATES {
        let features = match extra {
            ExtraSection::PublicFeatures => public_features(package),
            _ => Vec::new(),
        };
        let feature_csv = features.join(",");
        let mut sections: Vec<(&str, Vec<&str>)> = vec![("default features", vec![])];
        match extra {
            ExtraSection::None => {}
            ExtraSection::PublicFeatures => sections.push((
                "all features except _*",
                if features.is_empty() {
                    vec![]
                } else {
                    vec!["--features", &feature_csv]
                },
            )),
            ExtraSection::Pinned(label, csv) => sections.push((label, vec!["--features", csv])),
        }

        let mut doc = String::new();
        doc.push_str(&format!(
            "# {package} public API surface\n\
             # Generated by tests/public_api_doc.rs via `cargo public-api --simplified`\n\
             # (regenerated on every `cargo test`; ZEN_API_DOC=check verifies, =off skips).\n"
        ));
        if matches!(extra, ExtraSection::PublicFeatures) {
            doc.push_str(
                "# Underscore-prefixed features are internal and excluded from the\n\
                 # all-features section.\n",
            );
        }
        doc.push_str(
            "# DO NOT EDIT BY HAND — commit regenerated changes together with the code.\n",
        );
        for (label, extra_args) in &sections {
            let items = surface(package, extra_args);
            doc.push_str(&format!("\n## {label} ({} items)\n\n", items.len()));
            for line in &items {
                doc.push_str(line);
                doc.push('\n');
            }
            eprintln!("{package} [{label}]: {} public items", items.len());
        }

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("docs/public-api")
            .join(format!("{package}.txt"));
        let existing = std::fs::read_to_string(&path).ok();

        if check {
            assert_eq!(
                existing.as_deref(),
                Some(doc.as_str()),
                "committed public-API snapshot for {package} is stale: run \
                 `cargo test` locally and commit the regenerated {}",
                path.display()
            );
        } else if existing.as_deref() != Some(doc.as_str()) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &doc).unwrap();
            eprintln!(
                "regenerated {} — review and commit the diff",
                path.display()
            );
        }
    }
}
