# Mangrove dev commands. Run `just` to list recipes, `just ci` for the full gate.

# Show available recipes
default:
    @just --list

# Format all crates (writes changes)
fmt:
    cargo fmt --all

# Check formatting without writing (CI gate)
fmt-check:
    cargo fmt --all --check

# Lint with clippy, warnings as errors
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Build the whole workspace
build:
    cargo build --workspace --locked

# Run all tests
test:
    cargo test --workspace --locked

# Full CI gate: format check, lint, build, test (single source of truth, mirrored by CI)
ci: fmt-check lint build test
