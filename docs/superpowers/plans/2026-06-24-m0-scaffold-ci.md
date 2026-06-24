# M0 — Scaffold + CI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Mangrove Cargo workspace, two stub crates (`mangrove-core`, `mangrove-cli`), a conformance-harness crate, and a single CI workflow (fmt → clippy → build → test) — all green, with a committed `Cargo.lock`.

**Architecture:** A virtual Cargo workspace under `crates/`. Crates are created lazily per milestone; M0 creates only `mangrove-core` (shared types), `mangrove-cli` (the `mangrove` binary), and `mangrove-conformance` (the spec §13 corpus runner). Shared package metadata and lints live once at the workspace root and are inherited by every crate.

**Tech Stack:** Rust (edition 2024, stable ≥ 1.85), Cargo workspaces, GitHub Actions.

## Global Constraints

- **Edition:** `2024` for every crate, inherited via `edition.workspace = true`.
- **License:** `Apache-2.0` (matches repo `LICENSE`), inherited via `license.workspace = true`.
- **Workspace inheritance:** version, edition, license, rust-version set once in `[workspace.package]`; lints set once in `[workspace.lints]`; crates opt in with `*.workspace = true` and `[lints] workspace = true`.
- **No unsafe:** `unsafe_code = "forbid"` at the workspace level.
- **Lints are errors:** clippy denied workspace-wide and `-D warnings` in CI.
- **Reproducible builds:** `Cargo.lock` is committed; CI uses `--locked`.
- All paths below are relative to the git repo root (`mangrove/`).

---

### Task 1: Workspace root + `mangrove-core` stub

**Files:**
- Create: `Cargo.toml` (workspace root, virtual manifest)
- Create: `crates/mangrove-core/Cargo.toml`
- Create: `crates/mangrove-core/src/lib.rs`
- Create: `crates/mangrove-core/src/error.rs`
- Create: `.gitignore`
- Create (generated, then committed): `Cargo.lock`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: crate `mangrove-core` exporting `mangrove_core::error::ValidationError` with `ValidationError::new(path: impl Into<String>, message: impl Into<String>) -> ValidationError` and public fields `path: String`, `message: String`.

- [ ] **Step 1: Write the failing test**

Create `crates/mangrove-core/src/error.rs`:

```rust
//! Structured validation error (spec §12). Fleshed out in later milestones;
//! at M0 it carries just a field path and a message.

/// A single structured validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dotted field path, e.g. `container.port`.
    pub path: String,
    /// Human- and machine-readable failure message.
    pub message: String,
}

impl ValidationError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_path_and_message() {
        let e = ValidationError::new("container.port", "out of range");
        assert_eq!(e.path, "container.port");
        assert_eq!(e.message, "out of range");
    }
}
```

Create `crates/mangrove-core/src/lib.rs`:

```rust
//! Core types shared across all Mangrove layers. No dependency on any layer.

pub mod error;
```

- [ ] **Step 2: Create the manifests**

Create workspace root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "Apache-2.0"
repository = "https://github.com/morrieinmaas/mangrove"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
```

Create `crates/mangrove-core/Cargo.toml`:

```toml
[package]
name = "mangrove-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lints]
workspace = true
```

Create `.gitignore`:

```gitignore
/target
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p mangrove-core`
Expected: PASS — `test error::tests::carries_path_and_message ... ok`. (This also generates `Cargo.lock`.)

- [ ] **Step 4: Verify formatting and lints are clean**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings`
Expected: both exit 0, no output from clippy.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore crates/mangrove-core
git commit -m "feat(core): scaffold workspace and mangrove-core stub"
```

---

### Task 2: `mangrove-cli` binary with `--version`

**Files:**
- Create: `crates/mangrove-cli/Cargo.toml`
- Create: `crates/mangrove-cli/src/main.rs`
- Test: `crates/mangrove-cli/tests/cli.rs`

**Interfaces:**
- Consumes: the workspace from Task 1.
- Produces: a binary target named `mangrove` (Cargo exposes its path to integration tests as `env!("CARGO_BIN_EXE_mangrove")`). `mangrove --version` prints `mangrove <version>\n` to stdout and exits 0; any other invocation prints usage to stderr and exits 2.

- [ ] **Step 1: Write the failing test**

Create `crates/mangrove-cli/tests/cli.rs`:

```rust
use std::process::Command;

#[test]
fn version_flag_prints_name_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("--version")
        .output()
        .expect("failed to run mangrove");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(stdout.starts_with("mangrove "), "stdout was {stdout:?}");
}

#[test]
fn unknown_args_exit_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("frobnicate")
        .output()
        .expect("failed to run mangrove");
    assert_eq!(out.status.code(), Some(2));
}
```

Create `crates/mangrove-cli/Cargo.toml`:

```toml
[package]
name = "mangrove-cli"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "mangrove"
path = "src/main.rs"

[lints]
workspace = true
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mangrove-cli`
Expected: FAIL — compilation error, `main.rs` does not exist yet (no binary to run).

- [ ] **Step 3: Write the minimal implementation**

Create `crates/mangrove-cli/src/main.rs`:

```rust
//! The `mangrove` command-line tool. At M0 it only reports its version;
//! subcommands (`hash`, `validate`, `build`) arrive with later milestones.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => {
            println!("mangrove {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!("usage: mangrove --version");
            std::process::exit(2);
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mangrove-cli`
Expected: PASS — both `version_flag_prints_name_and_version` and `unknown_args_exit_nonzero` ok.

- [ ] **Step 5: Commit**

```bash
git add crates/mangrove-cli Cargo.lock
git commit -m "feat(cli): add mangrove binary with --version"
```

---

### Task 3: `mangrove-conformance` harness skeleton + first vector

**Files:**
- Create: `crates/mangrove-conformance/Cargo.toml`
- Create: `crates/mangrove-conformance/src/lib.rs`
- Create: `crates/mangrove-conformance/tests/corpus.rs`
- Create: `tests/conformance/l0/smoke.mang`
- Create: `tests/conformance/l0/smoke.expected`

**Interfaces:**
- Consumes: the workspace from Task 1.
- Produces: `mangrove_conformance::vector_pairs(dir: &std::path::Path) -> Vec<(std::path::PathBuf, std::path::PathBuf)>` — returns sorted `(input.mang, input.expected)` pairs in `dir`, panicking if any `.mang` lacks a sibling `.expected`. M1 will add a `run_vector` that parses, canonicalizes, hashes, and compares against the `.expected` contents.

- [ ] **Step 1: Write the failing test**

Create `crates/mangrove-conformance/src/lib.rs`:

```rust
//! Conformance corpus runner (spec §13).
//!
//! At M0 this only *discovers* vector pairs and checks that every `.mang`
//! input has a matching `.expected` file. M1 adds the parse → canonical form
//! → CBOR → BLAKE3 pipeline and compares the produced `b3:` hash against the
//! `.expected` contents.

use std::fs;
use std::path::{Path, PathBuf};

/// Returns the `(input.mang, input.expected)` vector pairs in `dir`, sorted by
/// input path. Panics if a `.mang` file has no sibling `.expected`.
pub fn vector_pairs(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("mang") {
            continue;
        }
        let expected = path.with_extension("expected");
        assert!(expected.exists(), "missing .expected for vector {path:?}");
        pairs.push((path, expected));
    }
    pairs.sort();
    pairs
}
```

Create `crates/mangrove-conformance/tests/corpus.rs`:

```rust
use mangrove_conformance::vector_pairs;
use std::path::Path;

/// Absolute path to the L0 vector directory at the workspace root.
const L0_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/l0");

#[test]
fn every_l0_input_has_an_expected_file() {
    let pairs = vector_pairs(Path::new(L0_CORPUS));
    assert!(!pairs.is_empty(), "no conformance vectors found under {L0_CORPUS}");
}
```

Create `crates/mangrove-conformance/Cargo.toml`:

```toml
[package]
name = "mangrove-conformance"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mangrove-conformance`
Expected: FAIL — `vector_pairs` panics with `read_dir … No such file or directory` (the corpus directory does not exist yet).

- [ ] **Step 3: Add the first vector fixture**

Create `tests/conformance/l0/smoke.mang`:

```
{ name: "smoke", replicas: 1 }
```

Create `tests/conformance/l0/smoke.expected` (placeholder; M1's L0 task overwrites this with the real `b3:` content hash once the canonicalizer + hasher exist):

```
pending-m1
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mangrove-conformance`
Expected: PASS — `every_l0_input_has_an_expected_file ... ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mangrove-conformance tests/conformance Cargo.lock
git commit -m "feat(conformance): add corpus discovery harness and first L0 vector"
```

---

### Task 4: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the full workspace from Tasks 1–3.
- Produces: a CI pipeline running fmt → clippy → build → test on push and PR.

- [ ] **Step 1: Verify the full pipeline passes locally first**

Run (this is exactly what CI will run):

```bash
cargo fmt --all --check \
  && cargo clippy --all-targets --all-features -- -D warnings \
  && cargo build --workspace --locked \
  && cargo test --workspace --locked
```

Expected: all four commands exit 0; test output shows the `mangrove-core`, `mangrove-cli`, and `mangrove-conformance` tests passing.

- [ ] **Step 2: Write the workflow file**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all --check
      - name: Clippy
        run: cargo clippy --all-targets --all-features -- -D warnings
      - name: Build
        run: cargo build --workspace --locked
      - name: Test
        run: cargo test --workspace --locked
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add fmt/clippy/build/test workflow"
```

- [ ] **Step 4: Push and verify CI is green**

```bash
git push
gh run watch
```

Expected: the `check` job passes all four steps. (Skip if no GitHub remote is configured; the local run in Step 1 is the gate.)

---

## Self-Review

**Spec coverage (against the roadmap's M0 definition):**
- Workspace root + `[workspace.package]`/`[workspace.lints]` → Task 1 ✓
- `mangrove-core` stub → Task 1 ✓
- `mangrove-cli` stub → Task 2 ✓
- Conformance harness skeleton + `tests/conformance/` → Task 3 ✓
- Committed `Cargo.lock` → generated in Task 1, committed in every task ✓
- Single CI file (fmt/clippy/build/test, `--locked`) → Task 4 ✓
- Lazy crate creation (no empty layer stubs) → only core/cli/conformance created ✓

**Placeholder scan:** The only literal placeholder is `tests/conformance/l0/smoke.expected` containing `pending-m1` — this is intentional test-fixture content, explicitly owned by M1's L0 task (which replaces it with the real hash), not an unfinished plan step. The M0 harness checks file *existence*, not contents, so the fixture is fully exercised.

**Type consistency:** `ValidationError::new` / `.path` / `.message` (Task 1) and `vector_pairs(&Path) -> Vec<(PathBuf, PathBuf)>` (Task 3) are used consistently; the CLI binary name `mangrove` matches `CARGO_BIN_EXE_mangrove` in the Task 2 test.
