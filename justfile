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

# Fail if any graph root's lock holds more than one copy of the zencodec core
# family (zencodec, zencodec-io, zencodec-coroutine, zenmeta).
check-core-family:
    python3 scripts/check-core-family.py --self-test
    python3 scripts/check-core-family.py

# Swap one native repository's git pin everywhere it is named, re-resolve every
# lock, and stop on any unrelated package movement (left in place for review).
repin-native repo old new crates:
    bash scripts/repin-native.sh {{repo}} {{old}} {{new}} {{crates}}

# Strict Clippy on the single-codec zencodecs subset each zenpipe `nodes-<codec>`
# feature builds; every node feature by default, or NODE=CODEC pairs
# (`just clippy-codec-subsets nodes-jxl=jxl-decode`).
clippy-codec-subsets *pairs:
    bash scripts/clippy-codec-subsets.sh {{pairs}}

# Synthetic controls for the qualification runner in scripts/qualify (stub
# cargo and stub test binaries; builds nothing). Every failure mode must fail.
qualify-selftest:
    bash scripts/qualify/runner-selftest.sh

# zencodecs' stop-latency benchmarks (#[ignore]d, observational), each alone in
# an optimized build: how long a codec takes to return after its stop fires.
# AVIF needs zenavif, so the default list leaves its two benchmarks out.
stop-latency *tests="stop_latency_jpeg_decode stop_latency_webp_decode stop_latency_png_decode stop_latency_gif_decode stop_latency_jpeg_encode stop_latency_webp_encode stop_latency_png_encode stop_latency_gif_encode":
    for t in {{tests}}; do cargo test --release -p zencodecs --no-default-features --features std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp --test stop_latency --locked -- "$t" --exact --ignored --nocapture --test-threads 1 || exit 1; done

# Run all CI checks locally
ci: fmt-check clippy clippy-codec-subsets check-pins check-core-family test fuzz-check fuzz-regression

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

# Minimal checked-transcode feature combination must remain warning-free.
checked-transcode-baseline:
    cargo clippy -p zencodecs --no-default-features --features std,cms,metadata,jpeg,png --lib -- -D warnings

# The joint-policy implementation and a real JPEG-to-PNG pixel oracle.
checked-transcode-check:
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode --test checked_transcode --locked
    cargo clippy -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode --lib --test checked_transcode --example checked_transcode --locked -- -D warnings

checked-transcode-sync export:
    test ! -e "{{export}}/.git"
    test ! -e "{{export}}/.jj"
    mkdir -p "{{export}}/zencodecs/src/checked_transcode"
    cp zencodecs/src/checked_transcode.rs zencodecs/src/lib.rs zencodecs/src/dispatch.rs zencodecs/src/dyn_dispatch.rs zencodecs/src/encode.rs zencodecs/src/gainmap.rs "{{export}}/zencodecs/src/"
    cp zencodecs/src/checked_transcode/*.rs "{{export}}/zencodecs/src/checked_transcode/"
    cp zencodecs/tests/checked_transcode.rs "{{export}}/zencodecs/tests/"
    mkdir -p "{{export}}/zencodecs/tests/checked_transcode"
    cp zencodecs/tests/checked_transcode/*.rs "{{export}}/zencodecs/tests/checked_transcode/"
    cp zencodecs/examples/checked_transcode.rs "{{export}}/zencodecs/examples/"
    cp zencodecs/src/metadata/mod.rs zencodecs/src/metadata/jpeg.rs "{{export}}/zencodecs/src/metadata/"
    cp zencodecs/Cargo.toml "{{export}}/zencodecs/Cargo.toml"
    cp Cargo.toml "{{export}}/Cargo.toml"
    cp fuzz/Cargo.toml "{{export}}/fuzz/Cargo.toml"
    cp zencodecs/fuzz/Cargo.toml "{{export}}/zencodecs/fuzz/Cargo.toml"
    cp demo/crate/Cargo.toml "{{export}}/demo/crate/Cargo.toml"
    cp tools/metadata-services/Cargo.toml "{{export}}/tools/metadata-services/Cargo.toml"
    cp justfile "{{export}}/justfile"

# Resolve pinned revisions outside local sibling overrides before testing.
checked-transcode-resolve:
    cargo update -p zencodecs
    cargo tree -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode -i zencodec

# The new operation must not remove existing adapter coverage.
checked-transcode-compat:
    cargo check -p zencodecs --no-default-features --features checked-transcode --locked
    cargo test -p zencodecs --no-default-features --all-targets --locked
    just test-gainmap-surface
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode --doc --locked
    just api-doc

# Preserve the decoder pin invariant across every standalone Cargo graph.
checked-transcode-locks:
    cargo update -p zencodecs
    cargo update --manifest-path fuzz/Cargo.toml -p zenpipe-fuzz -p magetypes -p archmage -p enough -p almost-enough
    cargo update --manifest-path zencodecs/fuzz/Cargo.toml -p zencodecs-fuzz -p magetypes -p archmage -p enough -p almost-enough -p rustix
    cargo update --manifest-path demo/crate/Cargo.toml -p zenpipe-demo -p magetypes -p archmage -p enough -p almost-enough
    cargo update --manifest-path tools/metadata-services/Cargo.toml -p zencodecs -p magetypes -p archmage -p enough -p almost-enough
    just check-pins

# Standalone application service and fuzz graphs use the same pinned contracts.
checked-transcode-integrations:
    cargo test --manifest-path tools/metadata-services/Cargo.toml --locked
    cargo check --manifest-path fuzz/Cargo.toml --all-targets --locked
    cargo check --manifest-path zencodecs/fuzz/Cargo.toml --all-targets --locked

# Final compatibility gates after changing the shared operation dependency.
checked-transcode-ci:
    cargo clippy -p zencodecs --no-default-features --features std,cms,metadata,jpeg,png,checked-transcode --all-targets --locked -- -D warnings
    cargo test -p zencodecs --no-default-features --features metadata,jpeg-ultrahdr,jxl-encode,jxl-decode,png --test metadata_scrub --locked
    just checked-transcode-check
    just api-doc

# Check the surrounding pipeline without launching standalone benchmark mains.
checked-transcode-workspace:
    cargo test -p zenfilters --lib --tests --locked
    cargo clippy -p zenfilters --all-targets --locked -- -D warnings
    cargo clippy --all-targets --no-default-features --locked -- -D warnings

checked-transcode-async-check:
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode-async --test checked_transcode --locked
    cargo clippy -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode-async --lib --test checked_transcode --example checked_transcode --locked -- -D warnings

checked-transcode-arm-clippy:
    cargo clippy -p zenfilters --all-targets --target aarch64-unknown-linux-gnu --locked -- -D warnings

# Compile all benchmark bodies, then exercise the native nested pipeline.
session-stack-check:
    cargo clippy --bench session_cache --locked -- -D warnings
    cargo test --bench session_cache --locked -- --group=session_4k_drain

# Qualify color handoff after resolving all pinned application graphs.
checked-color-integration:
    just checked-transcode-locks
    just checked-transcode-async-check
    just api-doc

# Run in an export; verifies one local review zencodec across registry/git edges.
checked-transcode-review-check codec:
    python3 scripts/check-review-codec.py "{{codec}}"

# Frame sizes at a stack-overflow abort in one io-check test (debug build, default
# 2 MiB test thread; no stack override). Example:
#   just io-stack-frames io_pipeline jpeg_controlled_strips_equal_the_one_shot_decode
io-stack-frames target test:
    #!/usr/bin/env bash
    set -euo pipefail
    exe=$(cargo test -p zenpipe-io-check --test "{{target}}" --no-run --locked --message-format=json 2>/dev/null \
      | jq -r 'select(.reason=="compiler-artifact" and .executable != null and .profile.test) | .executable')
    gdb -q -batch -x tools/io-check/scripts/stack_frames.py --args "$exe" "{{test}}" --exact --test-threads 1 --nocapture

# Run only after a serialized heavy slot is granted. Does not enter bench mains.
checked-publication-check:
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode-async --lib checked_transcode --locked
    cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode-async --test checked_transcode --locked
    cargo clippy -p zencodecs --no-default-features --features std,cms,jpeg,png,checked-transcode-async --lib --test checked_transcode --example checked_transcode --example checked_publication --locked -- -D warnings
    cargo check -p zencodecs --no-default-features --features checked-transcode --locked

# Source-only preflight; no Cargo command or native compilation.
checked-publication-source:
    timeout 45s rustfmt --check --edition 2024 --config skip_children=true zencodecs/src/checked_transcode.rs zencodecs/src/checked_transcode/execution.rs zencodecs/src/checked_transcode/asynchronous.rs zencodecs/src/checked_transcode/evidence.rs zencodecs/src/checked_transcode/publication.rs zencodecs/tests/checked_transcode.rs zencodecs/tests/checked_transcode/publication.rs zencodecs/tests/checked_transcode/async_source.rs zencodecs/examples/checked_publication.rs
    timeout 45s python3 scripts/check-decoder-pins.py
    just --show checked-publication-check > /dev/null

# Resolves only after a serialized heavy slot is granted, outside sibling overrides.
checked-publication-locks:
    timeout 180s cargo metadata --format-version 1 --manifest-path Cargo.toml > /dev/null
    timeout 180s cargo metadata --format-version 1 --manifest-path fuzz/Cargo.toml > /dev/null
    timeout 180s cargo metadata --format-version 1 --manifest-path demo/crate/Cargo.toml > /dev/null
    timeout 180s cargo metadata --format-version 1 --manifest-path tools/metadata-services/Cargo.toml > /dev/null
    timeout 180s cargo metadata --format-version 1 --manifest-path zencodecs/fuzz/Cargo.toml > /dev/null
    python3 scripts/check-core-family.py
    python3 scripts/check-decoder-pins.py

# Full API/profile + existing source/browser controls, after a fresh slot grant.
checked-publication-final results core revision="":
    bash scripts/qualify/publication-final.sh "{{results}}" "{{core}}" "{{revision}}"

# Frozen graph migration; a fresh serialized heavy grant is required.
checked-publication-graph results core expected:
    bash scripts/qualify/publication-graph.sh "{{results}}" "{{core}}" "{{expected}}"

# Node protocol controls only; browser/pixel readback is a separate granted gate.
checked-preview-source:
    timeout 45s node --test demo/tests-node/worker-preview.test.cjs
    timeout 45s node --input-type=module --check < demo/js/export-modal.js
    timeout 45s node --check demo/tests/encode-preview.spec.js

# Reads retained locks/metadata only; no Cargo invocation.
checked-publication-delta-source frozen:
    timeout 45s python3 scripts/qualify/publication-delta-controls.py "{{frozen}}"

# FIRST compiler gate after a fresh grant; keeps the full canonical API features.
checked-publication-diagnostic results:
    bash scripts/qualify/publication-diagnostic.sh "{{results}}"

# Optional per-call control, effect boundaries and legacy cancellation adapters.
pipeline-pulse-check:
    cargo test -p zenpipe --no-default-features --test limits --locked
    cargo clippy -p zenpipe --no-default-features --lib --test limits --locked -- -D warnings
    cargo test -p zenpipe --no-default-features --features std,zennode --lib session:: --locked

# Native GIF forwarding/byte controls and retained-job cancellation.
animation-pulse-check:
    cargo test -p zenpipe --no-default-features --test animation --locked
    cargo clippy -p zenpipe --no-default-features --lib --test animation --locked -- -D warnings
    cargo test -p zenpipe --no-default-features --features std,job,nodes-gif --lib animation_control_tests --locked

# Lossless native JPEG and processing controls, including legacy adapters.
processing-pulse-check:
    cargo test -p zenpipe --no-default-features --features std,lossless-jpeg --test lossless_jpeg --locked
    cargo clippy -p zenpipe --no-default-features --features std,lossless-jpeg --lib --test lossless_jpeg --locked -- -D warnings
    cargo test -p zenfilters --lib pipeline::tests --locked
    cargo clippy -p zenfilters --lib --locked -- -D warnings

# Native consumer gate after portable AVIF source alignment.
avif-consumer-contract-check:
    cargo test --locked -p zencodecs --no-default-features --features avif-decode,avif-encode --test avif_capability -- --test-threads=1
    cargo clippy --locked -p zencodecs --no-default-features --features avif-decode,avif-encode --lib --test avif_capability -- -D warnings
