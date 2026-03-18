default:
    @just --list

# Run all tests (default features: std)
test:
    cargo test --all-targets
    cargo test --all-targets --no-default-features

# Run clippy
clippy:
    cargo clippy --all-targets -- -D warnings
    cargo clippy --all-targets --no-default-features -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all --check

# Run all CI checks locally
ci: fmt-check clippy test
