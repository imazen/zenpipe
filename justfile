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
    cargo clippy --locked --all-targets -- -D warnings
    cargo clippy --locked --all-targets --no-default-features -- -D warnings

# Format code + regenerate the public-API surface snapshots (docs/public-api/).
# The snapshot runner lives in the workspace-excluded apidoc/ package, so it
# is never built or run by plain `cargo test` or any CI job.
fmt:
    cargo fmt --all
    ZEN_API_DOC_TOOLCHAIN=nightly-2026-10-12 cargo test --manifest-path apidoc/Cargo.toml

# Regenerate the public-API surface snapshots only
api-doc:
    ZEN_API_DOC_TOOLCHAIN=nightly-2026-10-12 cargo test --manifest-path apidoc/Cargo.toml

# Verify the committed snapshots are current
api-doc-check:
    ZEN_API_DOC=check ZEN_API_DOC_TOOLCHAIN=nightly-2026-10-12 cargo test --manifest-path apidoc/Cargo.toml

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

# Exercise codec adapters on native ARM; JP2 remains a compile-error feature.
arm-codec-integration-audit:
    CARGO_BUILD_JOBS=4 RAYON_NUM_THREADS=4 OMP_NUM_THREADS=4 RUST_TEST_THREADS=4 TMPDIR="$HOME/tmp" nice -n19 cargo test -p zencodecs --no-fail-fast --features std,cms,tiff,svg,pdf-decode,heic-decode,raw-decode,bitmaps-hdr,bitmaps-qoi,bitmaps-tga

# Explicitly opt into real AVIF corpus tests; retain per-test output.
arm-avif-corpus-audit:
    CARGO_BUILD_JOBS=4 RAYON_NUM_THREADS=4 OMP_NUM_THREADS=4 RUST_TEST_THREADS=4 TMPDIR="$HOME/tmp" nice -n19 cargo test -p zencodecs --features std,cms,tiff,svg,pdf-decode,heic-decode,raw-decode,bitmaps-hdr,bitmaps-qoi,bitmaps-tga --test corpus -- --ignored avif --show-output

# Shared contracts through codec-owned rewriting; retains the original scrub cases.
metadata-dispatch-check:
    cargo test -p zencodecs --no-default-features --features metadata,jpeg-ultrahdr,jxl-encode,jxl-decode,png --test metadata_scrub
    cargo check -p zencodecs --no-default-features --features metadata
    cargo clippy -p zencodecs --no-default-features --features metadata,jpeg-ultrahdr,jxl-encode,jxl-decode,png --lib -- -D warnings

metadata-services-check:
    cargo test --manifest-path tools/metadata-services/Cargo.toml
    cargo check --manifest-path tools/metadata-services/Cargo.toml --features cli

# Validate the complete remote codec graph and optional AV1 backends.
metadata-codec-graph-check:
    cargo check -p zencodecs --features all,metadata,tiff,pdf-decode,avif-svt,avif-aom

metadata-inventory-dispatch-check:
    cargo test -p zencodecs --no-default-features --features inventory,jpeg,png,webp,gif,avif-encode,avif-decode,jxl-encode,jxl-decode --lib inventory::tests
    cargo check -p zencodecs-cli

metadata-final-api-check:
    cargo test -p zencodecs --features inventory,pdf-decode --test pdf pdf_inventory_dispatches_to_zenpdf
    cargo clippy -p zencodecs --features inventory,metadata --lib -- -D warnings
    cargo fmt -p zencodecs -p zencodecs-cli --check
    python3 scripts/check-decoder-pins.py
    just api-doc

metadata-consumer-regression-check:
    cargo test -p zenfilters --lib
    cargo check --manifest-path demo/crate/Cargo.toml
    cargo check --manifest-path zencodecs/fuzz/Cargo.toml --bin fuzz_metadata_scrub
    actionlint .github/workflows/ci.yml .github/workflows/demo-deploy.yml

# Match the SIMD feature configuration exercised by workspace CI.
metadata-filter-ci-check:
    cargo test -p zenfilters --lib
    cargo clippy -p zenfilters --all-targets -- -D warnings
    cargo check -p zenfilters --target aarch64-unknown-linux-gnu
    cargo fmt -p zenfilters --check

metadata-gif-consumer-check:
    cargo test -p zencodecs --test gif_capability
    cargo test -p zencodecs --lib animation
    cargo clippy -p zencodecs --lib -- -D warnings

metadata-codec-ci-check:
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp,raw-decode,raw-decode-exif,raw-decode-xmp,raw-decode-gainmap --all-targets

metadata-pin-check:
    python3 scripts/check-decoder-pins.py --self-test
    python3 scripts/check-decoder-pins.py

# Finite all-pairs catalog; ZEN_CODEC_MATRIX_REPORT optionally saves TSV output.
codec-operation-matrix:
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,webp,gif-zenquant,bitmaps-bmp,bitmaps-qoi,bitmaps-tga,bitmaps-hdr,jxl-encode,jxl-decode,avif-encode,avif-decode --test operation_matrix -- --nocapture

# Caller enables independent readers; missing programs fail the test.
codec-operation-matrix-oracles:
    ZEN_CODEC_MATRIX_MAGICK=magick ZEN_CODEC_MATRIX_DJXL=djxl just codec-operation-matrix

codec-operation-matrix-inventory:
    cargo test -p zencodecs --no-default-features --features std,cms,inventory,jpeg,png,webp,gif-zenquant,bitmaps-bmp,bitmaps-qoi,bitmaps-tga,bitmaps-hdr,jxl-encode,jxl-decode,avif-encode,avif-decode,heic-decode,raw-decode --test operation_matrix -- --nocapture

codec-operation-matrix-check:
    just codec-operation-matrix
    cargo clippy -p zencodecs --no-default-features --features std,cms,inventory,jpeg,png,webp,gif-zenquant,bitmaps-bmp,bitmaps-qoi,bitmaps-tga,bitmaps-hdr,jxl-encode,jxl-decode,avif-encode,avif-decode,heic-decode,raw-decode --test operation_matrix -- -D warnings
    cargo clippy -p zenfilters --target aarch64-unknown-linux-gnu --all-targets -- -D warnings

metadata-resolve-locks:
    #!/usr/bin/env bash
    set -euo pipefail
    for manifest in demo/crate/Cargo.toml fuzz/Cargo.toml tools/metadata-services/Cargo.toml zencodecs/fuzz/Cargo.toml; do
        cargo metadata --manifest-path "$manifest" --format-version 1 > /dev/null
    done
    just metadata-pin-check

metadata-arm-clippy:
    cargo clippy -p zenfilters --target aarch64-unknown-linux-gnu --all-targets -- -D warnings
# Test the dependency graph committed here without sibling source overrides.
check-streaming-pins:
    python3 scripts/check-streaming-pins.py --self-test
    python3 scripts/check-streaming-pins.py
    just check-pins

test-explicit-jpeg-config:
    cargo test --locked -p zencodecs --test explicit_jpeg_config

test-animation-output:
    cargo test --locked -p zencodecs --test animation_streamed_bytes -- --nocapture

test-strided-encode:
    cargo test --locked -p zencodecs --test strided_encode

test-encode-color-description:
    cargo test --locked -p zencodecs --features cms --test color_adapt

test-standalone-wasm:
    cargo check --locked --manifest-path demo/crate/Cargo.toml --target wasm32-unknown-unknown
    cargo check --locked --manifest-path wasm-size-shim/Cargo.toml --target wasm32-unknown-unknown --lib

# Match the reused CI's feature/package checks, preserving every failure.
streaming-local-matrix:
    #!/usr/bin/env bash
    result=0
    for recipe in streaming-root-tests streaming-codec-tests streaming-filter-tests streaming-transcode-matrix; do
        if ! just "$recipe"; then result=1; fi
    done
    exit "$result"

streaming-root-tests:
    just streaming-disk-floor
    cargo test --locked --all-targets
    just streaming-disk-floor
    cargo test --locked --all-targets --no-default-features
    just streaming-disk-floor
    cargo test --locked --features zennode
    just streaming-disk-floor
    cargo test --locked --features zennode,json-schema
    just streaming-disk-floor
    cargo test --locked -p zenlayout --all-targets
    just streaming-disk-floor
    cargo test --locked -p zeneditor --all-targets

streaming-codec-tests:
    just streaming-disk-floor
    cargo test --locked -p zencodecs --no-default-features --all-targets
    just streaming-disk-floor
    cargo test --locked -p zencodecs --no-default-features --features std,pdf-decode --lib pdf_
    just streaming-disk-floor
    cargo test --locked -p zencodecs --no-default-features --features std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp,raw-decode,raw-decode-exif,raw-decode-xmp,raw-decode-gainmap --all-targets --no-fail-fast

streaming-filter-tests:
    just streaming-disk-floor
    cargo test --locked -p zenfilters --all-targets

streaming-transcode-matrix:
    just streaming-disk-floor
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked --release -p zencodecs --features jxl-encode,bitmaps-qoi,bitmaps-tga,bitmaps-hdr --test transcode_matrix

# Exercise explicit HDR-to-GIF clipping and native refusal with a small graph.
test-hdr-gif:
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,jpeg,png,webp,gif,gif-zenquant,bitmaps-hdr --test transcode_matrix

# Independent ImageMagick pixel oracle; missing tools fail this explicit gate.
test-hdr-gif-oracle:
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,jpeg,png,webp,gif,gif-zenquant,bitmaps-hdr,local-fixtures --test transcode_matrix hdr_white_to_gif

clippy-hdr-gif:
    cd "{{env_var('HOME')}}/tmp" && cargo clippy --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,jpeg,png,webp,gif,gif-zenquant,bitmaps-hdr --test transcode_matrix -- -D warnings

streaming-clippy-gate:
    just clippy
    just streaming-disk-floor
    cargo clippy --locked -p zencodecs --features jxl-encode,tiff,bitmaps-qoi,bitmaps-tga,bitmaps-hdr --all-targets -- -D warnings
    just streaming-disk-floor
    cargo clippy --locked -p zencodecs --no-default-features --all-targets -- -D warnings
    just streaming-disk-floor
    cargo clippy --locked -p zenlayout --all-targets -- -D warnings
    just streaming-disk-floor
    cargo clippy --locked -p zeneditor --all-targets -- -D warnings
    just streaming-disk-floor
    cargo clippy --locked -p zenfilters --all-targets -- -D warnings

streaming-doc-baseline:
    cargo doc --locked --no-deps -p zenpipe -p zencodecs
    cargo test --locked --doc -p zenpipe -p zencodecs

# Compare unpublished crates with an explicit reviewed source export.
semver-against baseline:
    just semver-json-against "{{baseline}}" "{{env_var('HOME')}}/tmp/zenpipe-semver"

# Keep the shared-machine matrix above its disk-space floor.
streaming-disk-floor:
    python3 -c 'import shutil, sys; from pathlib import Path; free = shutil.disk_usage(Path.home()).free; print("available disk bytes:", free); sys.exit(0 if free >= 8 * 1024**3 else "below the 8 GiB floor; reclaim inactive Cargo output first")'

# Check strict color before and after format negotiation, with and without CMS.
test-strict-color:
    cargo test --locked -p zencodecs --test color_fit
    cargo test --locked -p zencodecs --features cms --test color_fit

# Complete only the matrix commands stopped by the disk-space floor.
streaming-local-tail:
    #!/usr/bin/env bash
    result=0
    for recipe in streaming-filter-tests streaming-transcode-matrix; do
        if ! just "$recipe"; then result=1; fi
    done
    exit "$result"

# Complete-file byte identity for all dispatched encoders.
test-dispatched-output:
    cargo test --locked -p zencodecs --features jxl-encode,tiff,bitmaps-qoi,bitmaps-tga,bitmaps-hdr --test streamed_bytes -- --nocapture

# Independent reader smoke tests; missing tools fail this explicit opt-in gate.
test-dispatched-oracle:
    cargo test --locked -p zencodecs --features local-fixtures,jxl-encode,tiff,bitmaps-qoi,bitmaps-tga,bitmaps-hdr --test streamed_bytes independent_decoders_read_every_streamed_format -- --nocapture

# Include limits smaller than an encoder header.
test-dispatched-output-limits:
    cargo test --locked -p zencodecs --features jxl-encode,tiff,bitmaps-qoi,bitmaps-tga,bitmaps-hdr --test streamed_bytes output_budgets -- --nocapture

# Preserve root patches when unpublished dependencies prevent an outer-workspace build.
semver-json-against baseline out:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{out}}"
    for which in baseline current; do
        source="{{baseline}}"
        if [ "$which" = current ]; then source="$PWD"; fi
        for package in zenpipe zencodecs; do
            just streaming-disk-floor
            features=()
            if [ "$package" = zencodecs ]; then features=(--features jxl-encode,cms); fi
            RUSTC_BOOTSTRAP=1 cargo +stable rustdoc --locked --manifest-path "$source/Cargo.toml" -p "$package" --lib "${features[@]}" -- -Z unstable-options --output-format json --cap-lints allow
            cp "$CARGO_TARGET_DIR/doc/$package.json" "{{out}}/$which-$package.json"
        done
    done
    for package in zenpipe zencodecs; do
        cargo semver-checks check-release --current-rustdoc "{{out}}/current-$package.json" --baseline-rustdoc "{{out}}/baseline-$package.json" --release-type patch
    done

# Sink aborts, buffered fallback, strided views and explicit native configuration.
test-streaming-seam:
    cargo test --locked --test sink_output_contract
    cargo test --locked -p zencodecs --test transcode_output_contract --test buffered_encode_contract --test strided_encode --test explicit_jpeg_config

# Schema fixtures must use the caller's configured temporary directory.
test-schema-export:
    cargo test --locked --features zennode,json-schema --lib schema_export

# Explicit HDR rendering, chunk parity, color precedence and strict-policy gates.
test-tonemap-policy:
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --lib tonemap
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --test tonemap --test color_fit

clippy-tonemap-policy:
    cd "{{env_var('HOME')}}/tmp" && cargo clippy --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --lib --test tonemap --test color_fit -- -D warnings

# Exercise cfg boundaries and preexisting color fitting alongside rendering.
test-tonemap-modes:
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --lib
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --test color_fit
    cd "{{env_var('HOME')}}/tmp" && cargo check --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features tonemap
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --doc

# Selected native encoder settings override the generic preference.
test-streaming-preference:
    cargo test --locked -p zencodecs --test prefer_streaming

# Verify the policy adapter and the independently merged streaming preference.
test-tonemap-integration: test-tonemap-policy clippy-tonemap-policy
    cd "{{env_var('HOME')}}/tmp" && cargo test --manifest-path "{{justfile_directory()}}/Cargo.toml" --locked -p zencodecs --no-default-features --features std,cms,tonemap,bitmaps-bmp,png,gif,gif-zenquant,jpeg,webp --test prefer_streaming

# Merge verification combines metadata integration with main's newer behavior gates.
metadata-merge-check:
    cargo fmt -p zenpipe -p zencodecs -p zenfilters -p zenlayout -p zenpipe-cmd -p zeneditor -p zencodecs-cli --check
    just metadata-dispatch-check
    just test-streaming-preference
    just test-tonemap-integration
    just metadata-services-check
    just metadata-inventory-dispatch-check
    just check-streaming-pins

metadata-merge-resolve:
    cargo metadata --format-version 1 > /dev/null
    cargo metadata --manifest-path tools/metadata-services/Cargo.toml --format-version 1 > /dev/null
    cargo metadata --manifest-path zencodecs/fuzz/Cargo.toml --format-version 1 > /dev/null
    cargo metadata --manifest-path fuzz/Cargo.toml --format-version 1 > /dev/null
    cargo metadata --manifest-path demo/crate/Cargo.toml --format-version 1 > /dev/null
    cargo metadata --manifest-path wasm-size-shim/Cargo.toml --format-version 1 > /dev/null
    cargo fmt -p zenpipe -p zencodecs -p zenfilters -p zenlayout -p zenpipe-cmd -p zeneditor -p zencodecs-cli

# Advance only the codec sources whose inventory/metadata adapters are integrated.
metadata-merge-update-codecs:
    cargo update -p heic -p zenraw -p zenjxl -p jxl-encoder -p zenjxl-decoder -p zentiff -p zenpdf -p zenbitmaps -p zenpng -p ultrahdr-core
    just metadata-merge-resolve

# Default-feature and minimal builds keep the merged dispatch graph checked.
metadata-merge-workspace-check:
    just api-doc
    just api-doc-check
    just test
    just clippy
    cargo check --locked --manifest-path fuzz/Cargo.toml
    cargo check --locked --manifest-path zencodecs/fuzz/Cargo.toml

# Run the merged CI codec feature set and both independent-reader catalogs.
metadata-merge-codec-check:
    cargo test --locked -p zencodecs --no-default-features --features std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp,raw-decode,raw-decode-exif,raw-decode-xmp,raw-decode-gainmap,tonemap --all-targets
    just codec-operation-matrix-oracles
    ZEN_CODEC_MATRIX_MAGICK=magick ZEN_CODEC_MATRIX_DJXL=djxl just codec-operation-matrix-inventory
    just test-standalone-wasm

metadata-merge-matrix-lint:
    cargo fmt -p zencodecs --check
    cargo clippy --locked -p zencodecs --no-default-features --features std,cms,inventory,jpeg,png,webp,gif-zenquant,bitmaps-bmp,bitmaps-qoi,bitmaps-tga,bitmaps-hdr,jxl-encode,jxl-decode,avif-encode,avif-decode,heic-decode,raw-decode --test operation_matrix -- -D warnings

# Inspect a run without printing its complete step log.
ci-status run repo="imazen/zenpipe":
    gh run view "{{run}}" -R "{{repo}}" --json status,conclusion,jobs --jq '{status,conclusion,jobs:[.jobs[] | {id:.databaseId,name,status,conclusion}]}'

# Stream CI progress to a caller-owned log when waiting for a required gate.
ci-watch run repo="imazen/zenpipe":
    gh run watch "{{run}}" -R "{{repo}}" --interval 30 --exit-status

# Use a fresh output path to preserve prior diagnostic captures.
ci-job-log job out repo="imazen/zenpipe":
    gh api "repos/{{repo}}/actions/jobs/{{job}}/logs" --allow-escape-sequences > "{{out}}"

# Verify file/stdin inventory parity and the RAW-capable audit binary.
inventory-cli-check:
    cargo fmt -p zencodecs-cli --check
    cargo clippy --locked -p zencodecs-cli --all-targets -- -D warnings
    cargo test --locked -p zencodecs-cli --test cli
    cargo build --locked -p zencodecs-cli --features zencodecs/raw-decode
