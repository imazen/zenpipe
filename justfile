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

# Tile-pyramid memory/time grid (zenpipe#24). Writes a TSV of peak live heap,
# allocation churn and wall per (size x tile x layout x store x threads x
# source class). See benchmarks/tile_pyramid_profile_2026-08-28.md for how to
# read it and how to pair it with heaptrack (Linux) / `sample` (macOS).
tile-profile out="benchmarks/tile_pyramid_profile_$(date +%Y-%m-%d).tsv":
    cargo build --release --example tile_pyramid_profile
    ./target/release/examples/tile_pyramid_profile --tsv-header > "{{out}}"
    for wh in "256 256" "1024 1024" "4096 4096" "8000 8000" "10000 1000" "40000 1000" "100000 600"; do \
        set -- $wh; \
        ./target/release/examples/tile_pyramid_profile --width $1 --height $2 \
            --tile 254 --store sink-only --layout dzi --repeat 5 | tail -1 >> "{{out}}"; \
    done
    for t in 128 254 512 1024; do \
        ./target/release/examples/tile_pyramid_profile --width 40000 --height 1000 \
            --tile $t --store sink-only --layout dzi --repeat 5 | tail -1 >> "{{out}}"; \
    done
    for l in dzi iiif zoomify gmaps; do \
        ./target/release/examples/tile_pyramid_profile --width 4096 --height 4096 \
            --tile 0 --store null --layout $l --repeat 5 | tail -1 >> "{{out}}"; \
    done
    for s in sink-only null mem fs zip; do \
        ./target/release/examples/tile_pyramid_profile --width 10000 --height 1000 \
            --tile 254 --store $s --layout dzi --repeat 3 | tail -1 >> "{{out}}"; \
    done
    for n in 1 2 4 8 12; do \
        ./target/release/examples/tile_pyramid_profile --width 10000 --height 1000 \
            --tile 254 --store fs --layout dzi --encode jpeg --threads $n --repeat 3 | tail -1 >> "{{out}}"; \
    done
    for src in callback jpeg materialized spool; do \
        ./target/release/examples/tile_pyramid_profile --width 8000 --height 8000 \
            --tile 254 --store sink-only --layout dzi --source $src | tail -1 >> "{{out}}"; \
    done
    @echo "wrote {{out}}"

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

# Both fuzz workspaces are standalone `[workspace] members = ["."]` packages
# excluded from the root, so `cargo check --all-targets` and `just clippy` at
# the repo root never touch them; without this they only ever get compiled by
# hand. `cargo check` rather than `cargo fuzz build` for the same reason CI
# does it: the drift this catches is type errors and resolution failures, and
# cargo-fuzz would cost a nightly toolchain plus a full sanitizer-instrumented
# codegen of the codec graph to catch the same class.
#
# `zencodecs/fuzz` path-patches five sibling checkouts (`../../../zenavif`,
# `zenbitmaps`, `zenjpeg`, `zenanalyze`, `ultrahdr`) plus `codec-corpus/crate`;
# they must be checked out next to this repo or its cell fails to resolve.
#
# Compile every fuzz target — what .github/workflows/fuzz.yml gates.
fuzz-check:
    cd fuzz && cargo check --all-targets
    cd zencodecs/fuzz && cargo check --all-targets

# Replay the committed crash seeds (zencodecs/fuzz/regression/) on stable.
fuzz-regression:
    cargo test -p zencodecs --no-default-features --features "std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp" --test fuzz_regression -- --nocapture

# A git dep with no `rev` re-resolves to whatever the branch points at, so the
# AVIF decoder under this repo (imazen/zenavif -> imazen/rav1d-safe) can change
# with no edit to any manifest — and no job here enables `avif-decode`, so
# nothing else would notice. The self-test runs first so a check that has
# stopped detecting anything fails loudly instead of passing vacuously.
# Audit a sibling repo with:
#   python3 scripts/check-decoder-pins.py --root ../zentone \
#       --expect https://github.com/imazen/zenavif=<rev>
#
# Fail if the AVIF-decoder git deps float, disagree, or sit dead as an unused patch
check-pins:
    python3 scripts/check-decoder-pins.py --self-test
    python3 scripts/check-decoder-pins.py

# Run all CI checks locally
ci: fmt-check clippy check-pins test fuzz-check fuzz-regression

# Build documentation site
site-build:
    zola --root site build

# Serve documentation site locally
site-serve:
    zola --root site serve --port 3100
