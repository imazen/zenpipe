default:
    @just --list

# Run all tests (default features: std)
test:
    cargo test --all-targets
    cargo test --all-targets --no-default-features

# Tests reading dev-workstation-only fixtures (sibling jpegli-cpp ICC
# profiles, /mnt/v corpora) — gated behind zencodecs `local-fixtures`.
test-local-fixtures:
    cargo test -p zencodecs --features local-fixtures --test icc_srgb

# zenfilters quality validation against libvips / darktable references on the
# CID22 corpus (zenpipe#44). Needs the corpus (`ZENFILTERS_CORPUS_DIR` or
# ../codec-corpus) plus `vips` and `darktable-cli` on PATH; missing
# prerequisites fail loudly. Never run on CI.
test-zenfilters-quality:
    cargo test -p zenfilters --features local-vips,local-darktable --test quality_validation

# Same, corpus + libvips only (no darktable-cli on the machine).
test-zenfilters-quality-vips:
    cargo test -p zenfilters --features local-fixtures,local-vips --test quality_validation

# zencodecs gain-map / UltraHDR / raw surface — the avif-less feature set
# mirrored by CI (zenpipe#38); widen to `all,cms,std` once the
# zencodec<->zenavif drift settles.
test-gainmap-surface:
    cargo test -p zencodecs --no-default-features --features "std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp,raw-decode,raw-decode-exif,raw-decode-xmp,raw-decode-gainmap" --all-targets

# Run clippy
clippy:
    cargo clippy --all-targets -- -D warnings
    cargo clippy --all-targets --no-default-features -- -D warnings

# Format code + regenerate the public-API surface snapshots (docs/public-api/).
# The snapshot runner lives in the workspace-excluded apidoc/ package, so it
# is never built or run by plain `cargo test` or any CI job.
fmt:
    cargo fmt --all
    cargo test --manifest-path apidoc/Cargo.toml

# Regenerate the public-API surface snapshots only
api-doc:
    cargo test --manifest-path apidoc/Cargo.toml

# Verify the committed snapshots are current
api-doc-check:
    ZEN_API_DOC=check cargo test --manifest-path apidoc/Cargo.toml

# Check formatting
fmt-check:
    cargo fmt --all --check

# Run all CI checks locally
ci: fmt-check clippy test

# Build documentation site
site-build:
    zola --root site build

# Serve documentation site locally
site-serve:
    zola --root site serve --port 3100
