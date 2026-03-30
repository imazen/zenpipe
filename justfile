# zencodecs justfile

check: fmt clippy test

fmt:
    cargo fmt

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all-features

build:
    cargo build --release --all-features

doc:
    cargo doc --all-features --no-deps --open

# Verify no_std compiles
check-no-std:
    cargo build --no-default-features --target wasm32-unknown-unknown

outdated:
    cargo outdated

# Feature permutation checks
feature-check:
    cargo test --no-default-features --features std
    cargo test --no-default-features --features "jpeg,png"
    cargo test --no-default-features --features "jpeg,webp,gif,png"

# Cross-compilation targets (use --no-default-features --features png to avoid path deps)
test-i686:
    cross test --no-default-features --features png --target i686-unknown-linux-gnu

test-armv7:
    cross test --no-default-features --features png --target armv7-unknown-linux-gnueabihf

test-cross: test-i686 test-armv7

# zcimg CLI
zcimg-build:
    cargo build --release --manifest-path zcimg/Cargo.toml

zcimg-run *ARGS:
    cargo run --release --manifest-path zcimg/Cargo.toml -- {{ARGS}}

# ═══════════════════════════════════════════════════════════
# Fuzzing (cross-platform: works on Linux, macOS, Windows)
# ═══════════════════════════════════════════════════════════

# Build all fuzz targets (release mode with debug info).
fuzz-build:
    cd fuzz && cargo +nightly fuzz build

# List all available fuzz targets.
fuzz-list:
    cd fuzz && cargo +nightly fuzz list

# Run a specific fuzz target.
# Usage: just fuzz <target> [extra-libfuzzer-args]
# Example: just fuzz fuzz_decode -- -max_total_time=60
fuzz TARGET *ARGS:
    cd fuzz && cargo +nightly fuzz run {{TARGET}} corpus/seed/mixed -- -dict=multiformat.dict {{ARGS}}

# Run a target for N seconds (default 60).
# Example: just fuzz-timed fuzz_decode 120
fuzz-timed TARGET DURATION="60":
    cd fuzz && cargo +nightly fuzz run {{TARGET}} corpus/seed/mixed -- -dict=multiformat.dict -max_total_time={{DURATION}}

# Run all high-priority fuzz targets for N seconds each (default 60).
fuzz-ci DURATION="60":
    just fuzz-timed fuzz_probe {{DURATION}}
    just fuzz-timed fuzz_decode {{DURATION}}
    just fuzz-timed fuzz_exif {{DURATION}}
    just fuzz-timed fuzz_decode_limits {{DURATION}}

# Run all fuzz targets for 60 seconds each (quick smoke test).
fuzz-smoke:
    just fuzz-timed fuzz_probe 60
    just fuzz-timed fuzz_decode 60
    just fuzz-timed fuzz_exif 60
    just fuzz-timed fuzz_decode_limits 60
    just fuzz-timed fuzz_push_decode 60
    just fuzz-timed fuzz_animation 60
    just fuzz-timed fuzz_transcode 60
    just fuzz-timed fuzz_gainmap 60
    just fuzz-timed fuzz_depthmap 60
    just fuzz-timed fuzz_roundtrip 60
    just fuzz-timed fuzz_select 60

# Run all fuzz targets for 30 minutes each (deep fuzzing).
fuzz-deep:
    just fuzz-timed fuzz_probe 1800
    just fuzz-timed fuzz_decode 1800
    just fuzz-timed fuzz_exif 1800
    just fuzz-timed fuzz_decode_limits 1800
    just fuzz-timed fuzz_push_decode 1800
    just fuzz-timed fuzz_animation 1800
    just fuzz-timed fuzz_transcode 1800
    just fuzz-timed fuzz_gainmap 1800
    just fuzz-timed fuzz_depthmap 1800
    just fuzz-timed fuzz_roundtrip 1800
    just fuzz-timed fuzz_select 1800

# Run a fuzz target with coverage instrumentation and generate a report.
fuzz-cov TARGET:
    cd fuzz && cargo +nightly fuzz coverage {{TARGET}} corpus/seed/mixed

# Seed fuzz corpus from codec-corpus conformance suites (cross-platform).
# Downloads on first use, caches locally. Works on Linux, macOS, Windows.
fuzz-seed:
    cd fuzz && cargo run --bin seed_corpus_tool

# Seed corpus from local sibling crates + external repos (Linux/macOS only).
[unix]
fuzz-seed-full *ARGS:
    ./fuzz/seed_corpus.sh {{ARGS}}

# Clean fuzz artifacts and coverage data (preserves corpus).
[unix]
fuzz-clean:
    rm -rf fuzz/target fuzz/artifacts fuzz/coverage

[windows]
fuzz-clean:
    if exist fuzz\target rmdir /s /q fuzz\target
    if exist fuzz\artifacts rmdir /s /q fuzz\artifacts
    if exist fuzz\coverage rmdir /s /q fuzz\coverage
