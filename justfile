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

# Format code + regenerate the public-API surface snapshots (docs/public-api/)
fmt:
    cargo fmt --all
    cargo test -p zenpipe --test public_api_doc

# Regenerate the public-API surface snapshots only
api-doc:
    cargo test -p zenpipe --test public_api_doc

# Verify the committed snapshots are current (what CI runs)
api-doc-check:
    ZEN_API_DOC=check cargo test -p zenpipe --test public_api_doc

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
