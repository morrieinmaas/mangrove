# M3a — L2 Local Composition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline). Steps use `- [ ]`. Design: `../specs/2026-06-24-m3a-l2-local-composition-design.md`. Decisions D19–D23.

**Goal:** Compose documents locally — `use ./base.mang`, `...spread` + last-wins deep-merge, `unset`, `@key` list ops, subtype redefinition — resolving to one merged value that validates/hashes like a hand-written one (D12).

**Architecture:** New `mangrove-compose` crate (L2 semantics). The document body becomes an ordered statement list (`Bind`/`Spread`) so composition can fold it; a doc with only plain binds folds to the same `Value::Map` as today (L0/L1 hashes unchanged). Pipeline: parse → **compose** → resolve → validate → hash.

**Tech Stack:** Rust 2024; reuses existing crates. No new external deps.

## Global Constraints

- Edition 2024, Apache-2.0, workspace inheritance, `unsafe_code = "forbid"`, clippy `-D warnings`. Gate per task: `just ci` green; clippy via `rtk proxy cargo clippy`.
- Commit directly to `main`. **L0/L1 hashes must not change** — a doc with no composition features folds to the identical `Value` (assert via existing conformance corpus).
- TDD: failing test first. Guard recursion depth (merge/subtype/load) — the earlier reviews found stack-overflow DoS in every recursive addition.

Order: (1) merge engine; (2) statement-based body + `unset` + spread parse; (3) local `use` + compose driver + CLI; (4) `@key` list ops + `+=`; (5) subtype redefinition; (6) conformance.

---

### Task 1: `mangrove-compose` crate + merge engine

**Files:** Create `crates/mangrove-compose/{Cargo.toml,src/lib.rs,src/merge.rs}`.

**Produces:** `mangrove_compose::merge(base: Value, over: Value) -> Value` — records/maps (both `Value::Map`) deep-merge key-by-key (recurse); a key whose `over` value is the unset marker is removed; every other shape (scalar/list/kind-mismatch) → `over` replaces (D20). Depth-guarded.

- [ ] **Step 1: Decide the unset marker** — add `Value::Unset` to `mangrove-core` (a marker that must never reach the encoder, like `Value::Unit`); add the CBOR guard arm `panic!` and a `render` arm; fix the non-exhaustive matches (validate/resolve treat a stray `Unset` like a kind error — it should be gone after compose). (This is the one cross-crate change; do it first so everything compiles.)
- [ ] **Step 2: Failing tests** (`merge.rs`): deep-merge two maps (overlapping + disjoint keys); scalar override; list replace; `unset` removes a key; nested map deep-merge; depth guard (a pathologically deep pair errors/:is bounded — merge returns, doesn't overflow; cap recursion at the shared `MAX_DEPTH`).
- [ ] **Step 3: Manifest + implement** `merge(base, over)`:
```rust
pub fn merge(base: Value, over: Value) -> Value { merge_d(base, over, 0) }
fn merge_d(base: Value, over: Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH { return over; } // bounded; deep configs are not real
    match (base, over) {
        (_, Value::Unset) => /* signal removal — see note */,
        (Value::Map(mut b), Value::Map(o)) => { for (k,v) in o { merge or remove k }; Value::Map(b) }
        (_, over) => over,
    }
}
```
  Note: removal needs a sentinel return or merge to operate on the *map* level. Simpler: `merge_into(&mut BTreeMap, key, over_value)` — if `over_value == Unset` remove key, else deep-merge/replace. Implement merge at the map-entry granularity so `unset` removal is natural; top-level `merge(base, over)` requires both be maps (the document body is always a map).
- [ ] **Step 4: Run** `cargo test -p mangrove-compose` + `cargo test --workspace` (exhaustive-match fixes compile) → PASS.
- [ ] **Step 5: Commit** `feat(compose): merge engine (deep-merge, unset removes, list replace)`.

---

### Task 2: Parse spread, `unset`, statement body

**Files:** `crates/mangrove-syntax/src/{lexer.rs,parser.rs}`.

**Produces:** `Tok::DotDotDot` (`...`); `unset` recognized as a value (→ `Value::Unset`); the document body parsed as an ordered `Vec<Stmt>` where `Stmt = Bind(String, Value) | Spread(String)`; `Document.body` stays a `Value` for pure docs but gains `Document.stmts: Vec<Stmt>` (or `body` becomes `Vec<Stmt>` and compose folds — see note). Nested map values may also contain spreads/unset.

- [ ] **Step 1: Failing tests** — lex `...`; `parse_document("...base\nx: 1")` yields a spread stmt then a bind; `k: unset` yields `Value::Unset`; a pure `a: 1\nb: 2` doc still yields the same map (regression).
- [ ] **Step 2: Implement** — lexer: `.` then `..` → `DotDotDot` (greedy; a single/double dot stays `Dot`). parser: at body-statement position, `...` + bareword → `Stmt::Spread(alias-or-path)`; `unset` bareword in value position → `Value::Unset`. Represent the body as `Vec<Stmt>`; a pure doc (all binds, no spread) still composes to the same `Value::Map`. Nested `{ ...x, k: unset }` likewise. Keep `parse` (→Value) working by folding a spread-free body.
- [ ] **Step 3: Run** `cargo test -p mangrove-syntax` (+ conformance L0/L1 unchanged) → PASS.
- [ ] **Step 4: Commit** `feat(syntax): parse spread (...), unset, and a statement-based body`.

---

### Task 3: Local `use` + compose driver + CLI

**Files:** `crates/mangrove-syntax` (`use ./p as a` statement → `Document.uses: Vec<(String, String)>`); `crates/mangrove-compose/src/load.rs`; `crates/mangrove-cli/src/main.rs`.

**Produces:** `mangrove_compose::compose(path: &Path) -> Result<Composed, ComposeError>` returning the merged body `Value` + merged `TypeEnv` (own + `alias.Name`), resolving local `use` recursively (cycle-checked, cached), folding statements (spread aliases + binds + unset) via the merge engine. CLI `hash`/`check` call `compose` first.

- [ ] **Step 1: Failing tests** — two temp files: `base.mang` (`name: "x"\nport: 8080`) and `over.mang` (`use ./base.mang as base\n...base\nport: 9090`); compose → `{name:"x", port:9090}`. Cyclic use → error. `...base` then `k: unset` removes an inherited key.
- [ ] **Step 2: Implement** `load` (parse file → recursively load its `use`s, cycle-detect via a visiting set, cache by canonical path) and `compose` (fold the importing doc's stmts: a `Spread(alias)` merges that alias's composed body; a `Bind` merges `{k: v}`). Local-path-only (D19): a namespaced `use` errors. CLI: `cmd_hash`/`cmd_check` → `compose(path)` → then resolve/validate/hash on the merged value + env.
- [ ] **Step 3: Run** `cargo test -p mangrove-compose -p mangrove-cli` + conformance → PASS.
- [ ] **Step 4: Commit** `feat(compose,cli): local use + compose driver wired into hash/check`.

---

### Task 4: `@key` list operations + `+=`

**Files:** `crates/mangrove-syntax` (`@key(field)` annotation; the `name { patch …, append: …, remove: … }` op block; `xs += [...]`); `crates/mangrove-compose` (apply ops during merge using the schema's `@key`).

- [ ] **Step 1: Failing tests** — base with `containers: [ {name:"api",…}, {name:"cron",…} ]`; overlay op block `patch "api"`, `append`, `remove "cron"` → expected merged list; `+=` appends; bare list replaces.
- [ ] **Step 2: Implement** — parse `@key(field)` (extend annotations or a dedicated field attr); parse the op block as a value form; in merge, when the schema field is `@key(f)`, apply patch (find by key, deep-merge), append (error on dup key), remove (drop by key); `+=` appends. Requires the schema in scope during merge (thread the field type / `@key` info).
- [ ] **Step 3: Run** tests + conformance → PASS.
- [ ] **Step 4: Commit** `feat(compose): @key list operations (patch/append/remove) and +=`.

---

### Task 5: Subtype redefinition `schema Base & {…}`

**Files:** `crates/mangrove-syntax` (parse `schema Name & { … }`); `crates/mangrove-compose/src/subtype.rs` (the `<:` checker).

**Produces:** `is_subtype(new: &Type, old: &Type, env) -> Result<(), String>` — structural/covariant/recursive (D23); a `schema Base & {over}` narrows `Base` and is admitted iff `New <: Old`, else a load error.

- [ ] **Step 1: Failing tests** — `int & >=1 & <=10 <: int` ok; `int <: int & <=10` NOT (loosening) → err; record field narrowing ok; required→optional forbidden; adding an unknown field forbidden; enum subset ok; regex non-identical rejected.
- [ ] **Step 2: Implement** `is_subtype` per §5.5 (interval containment, enum subset, structural records with required-ness monotonicity, covariant list/map, union drop/narrow; regex only if identical; `require` ignored in the check). Parse `schema Name & { fields }`; build the narrowed type; check `<:`; on success use the narrowed type as the schema.
- [ ] **Step 3: Run** tests + conformance → PASS.
- [ ] **Step 4: Commit** `feat(compose): subtype redefinition (schema Base & {…}, New <: Old)`.

---

### Task 6: Conformance — L2 vectors

**Files:** `crates/mangrove-conformance`; `tests/conformance/l2/`.

- [ ] **Step 1:** Add an L2 corpus + harness that composes a primary `.mang` (which `use`s siblings) and compares the resulting canonical hash / errors to `.expected`.
- [ ] **Step 2: Vectors** — spread+override hash; `unset` removal; `@key` patch/append/remove; subtype-redefinition accept (validates) and reject (load error). A compose-equals-handwritten hash-equivalence vector.
- [ ] **Step 3:** `just ci` green; push; verify CI.
- [ ] **Step 4: Commit** `feat(conformance): L2 composition vectors`.

---

## Self-Review

**Spec coverage:** merge/D20 → T1; spread/`unset`/D21 → T1/T2/T3; local `use`/D19 → T3; `@key`/`+=`/D22 → T4; subtype/D23 → T5; conformance → T6. ✓

**Hash-stability:** a composition-free document must fold to the identical `Value` and hash — asserted by the existing L0/L1 corpus on every task. `Value::Unset` never reaches the encoder (guarded), as `Value::Unit` is.

**Recursion guards:** merge, load (cycle + depth), and the subtype checker all bound recursion (the prior reviews found stack-overflow in every recursive feature).
