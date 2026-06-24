# M3b.1 — Resolver, Lockfile & Hash-Verify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps `- [ ]`. Design: `../specs/2026-06-24-m3b1-resolver-lockfile-design.md`. Decisions D24–D27.

**Goal:** Resolve namespaced `use "ns@tag" as alias` through `.mangrove/resolvers.toml` (local-dir backend) + a committed `mangrove.lock`, **hash-verifying source bytes before eval, failing closed**. Add `mangrove update`.

**Architecture:** New `mangrove-resolve` crate (config parsing + resolve + verify). `mangrove-compose` routes namespaced `use` through it (local `use` unchanged). Namespaced `use` already parses (M3a `parse_use`).

## Global Constraints

- Edition 2024, Apache-2.0, workspace inheritance, `unsafe_code = "forbid"`, clippy `-D warnings`.
- **Gate every commit on `just ci` passing first** (see memory — `cargo build` misses `-D warnings` lints).
- Commit directly to `main`. TDD. Bound recursion (compose already caps `use`-chain depth; namespaced imports go through the same `visiting` guard).
- Hermetic tests only — no network; the backend is a local directory.

New workspace dep: `toml = "0.8"`.

---

### Task 1: `mangrove-resolve` crate — config + resolve + verify

**Files:** Create `crates/mangrove-resolve/{Cargo.toml,src/lib.rs}`.

**Produces:**
- `Resolvers::find_and_load(root_dir: &Path) -> Result<Resolvers, String>` — search `root_dir` upward for `.mangrove/resolvers.toml`, parse `[namespace.<seg>] remote = "<dir>"` (via `toml::Table`); empty if none found.
- `Lockfile::find_and_load(root_dir) -> Result<Lockfile, String>` — load `mangrove.lock` (`"<ns>@<tag>" = "b3:…"`); empty if none.
- `resolve_path(resolvers, reference, config_dir) -> Result<PathBuf, String>` — split `reference` on last `@` into `ns_path` + `tag`; first segment of `ns_path` → namespace; look up `remote`; return `<remote>/<rest>.mang` (resolved relative to the resolvers.toml dir). Unknown namespace → error.
- `verify(bytes: &[u8], reference: &str, lock: &Lockfile) -> Result<(), String>` — `b3:` of `bytes` vs `lock[reference]`; missing → "not in lockfile; run `mangrove update`"; mismatch → "integrity check failed"; match → ok.
- `source_hash(bytes: &[u8]) -> String` — `"b3:" + blake3(bytes)` (source-bytes hash, D26).

- [ ] **Step 1: Manifest** with `toml`, `blake3` deps + `mangrove`-none (pure).
- [ ] **Step 2: Failing tests** — resolvers.toml parse (`[namespace.infra] remote="/x"`); resolve `"infra/k8s/core@v5.0"` → `<remote>/k8s/core.mang`; unknown ns → err; `verify` match/mismatch/missing; `source_hash` deterministic; upward search finds `.mangrove/resolvers.toml` in an ancestor.
- [ ] **Step 3: Implement** the above (hand types `Resolvers(HashMap<String,PathBuf>)`, `Lockfile(HashMap<String,String>)`; `toml::Table` for parsing; `blake3::hash`).
- [ ] **Step 4: Run** `cargo test -p mangrove-resolve` → PASS. Gate `just ci`.
- [ ] **Step 5: Commit** `feat(resolve): resolvers.toml + mangrove.lock parsing, resolve + hash-verify`.

---

### Task 2: compose integration — route namespaced `use` through the resolver

**Files:** `crates/mangrove-compose/{Cargo.toml,src/load.rs}`.

**Produces:** `compose_rec` routes a namespaced `use` (not `./`/`../`) through `mangrove-resolve`: find resolvers+lock from the root doc dir, `resolve_path`, read bytes, `verify` against the lock (fail closed), then recurse on the resolved file. Local `use` unchanged. Cycle/depth guards cover namespaced imports.

- [ ] **Step 1: Failing tests** (hermetic temp project): `root.mang` does `use "infra/x@v1" as k` + `...k`; a temp "remote" dir holds `x.mang`; `.mangrove/resolvers.toml` maps `infra` → that dir; `mangrove.lock` has the correct hash → composes. Tamper the remote source (hash mismatch) → integrity error. Missing lock entry → error.
- [ ] **Step 2: Implement** — thread the root dir into `compose_rec` (or load resolvers/lock once at the top and pass down). Replace the M3a "remote import errors" branch with: `resolve_path` → read bytes → `verify` → `compose_rec` on the resolved path. Carry `Resolvers`/`Lockfile` through the recursion.
- [ ] **Step 3: Run** `cargo test -p mangrove-compose` + `just ci` → PASS (local-use tests unchanged).
- [ ] **Step 4: Commit** `feat(compose): resolve + verify namespaced use through the resolver`.

---

### Task 3: `mangrove update` + CLI

**Files:** `crates/mangrove-cli/src/main.rs`; maybe a `write_lock` in `mangrove-resolve`.

**Produces:** `mangrove update <file>` resolves every reachable namespaced `use`, computes each source `b3:`, and writes/merges `mangrove.lock` (TOML). `hash`/`check` already compose (now resolving+verifying).

- [ ] **Step 1: Failing test** (CLI): a project with no `mangrove.lock`; `mangrove check root.mang` → error ("not in lockfile"); `mangrove update root.mang` writes the lock; then `mangrove check` → ok.
- [ ] **Step 2: Implement** `update`: walk the use-graph (reuse compose's resolution to enumerate references), hash each resolved source, serialize a sorted TOML lock, write `mangrove.lock` at the root. CLI dispatch for `update`.
- [ ] **Step 3: Run** `cargo test -p mangrove-cli` + `just ci` → PASS. Push; verify CI.
- [ ] **Step 4: Commit** `feat(cli): mangrove update writes the lockfile; resolve verified imports`.

---

## Self-Review

**Spec coverage:** config + resolve + verify (D25/D26/D27) → T1; compose integration + fail-closed → T2; `update` (lock writer) → T3. Namespaced `use` syntax (D24) already parses (M3a). Git backend + per-type pins → M3b.2. ✓

**Security focus:** T1/T2 prove verify-before-eval fails closed on tamper and missing entry — the load-bearing property. Hermetic (local-dir backend, no network).

**Guards:** namespaced imports go through compose's existing `visiting` cycle + `MAX_USE_DEPTH` guards (resolved path is the cycle key).
