# Mangrove — Implementation Roadmap (Rust)

> **Status:** Approved design — 2026-06-24
> **Scope:** Decomposition of the full Mangrove language (`mangrove-spec.md`, L0–L3) into sequenced, independently-shippable milestones, plus the workspace architecture, dependency choices, and CI that all milestones share.
> **Companions:** `../../../mangrove-spec.md` (normative), `../../../mangrove-rfc.md` (rationale).

This is a roadmap, not a single-feature design. Each milestone (M1+) gets its own spec → plan → TDD cycle. This document fixes the architecture and sequence so those cycles stay coherent.

---

## 1. Why decompose

Mangrove is a full language: lexer, parser, a typed value model, a decidable subtype checker, units-as-brands, composition/merge, value-layer templating, a deterministic canonical form (CBOR + BLAKE3), a git resolver + lockfile, and an executable conformance corpus. That is months of work and cannot be TDD'd as one unit. The spec's own L0→L1→L2→L3 layering is a strict, one-directional dependency chain, so it is also the natural decomposition: each layer is self-contained, builds only on the layers below it, and is independently testable.

---

## 2. Workspace architecture

A **Cargo workspace** (not a single crate). Rationale: the spec hands us the seams in advance (the layers), so the usual "boundaries are fluid, a workspace churns" objection does not apply. Crate boundaries give us **compiler-enforced** layering — L0 *cannot* depend on L1 — per-crate incremental compilation, and independent reusability (a second implementation can depend on just `mangrove-cbor` or `mangrove-core` to get byte-identical hashes, which the spec anticipates in §13). The spec's statement that an implementation "may target a subset of layers" becomes literally true: an L0/L1 reader never links L2/L3 code.

```
mangrove/                  # workspace root (virtual manifest)
  Cargo.toml               # [workspace], [workspace.dependencies], shared [workspace.lints]
  Cargo.lock               # committed — this is a tool/app, not a library
  crates/
    mangrove-core/         # value model, arbitrary-precision int/decimal, structured errors (§12). NO layer deps.
    mangrove-cbor/         # hand-written deterministic canonical CBOR encoder.        deps: core
    mangrove-canonical/    # §7 canonicalization (key sort, number/unit normalize) + BLAKE3 hash. deps: core, cbor
    mangrove-syntax/       # L0 lexer + parser + AST → core values.                    deps: core
    mangrove-typed/        # L1: schema, type grammar, refinements, units, match, require. deps: core, syntax, canonical
    mangrove-compose/      # L2: use/resolver/lockfile, spread+override, @key list ops, unset, subtype check. deps: typed
    mangrove-template/     # L3: params, fn, interpolation, emit.                       deps: compose
    mangrove-cli/          # `mangrove` binary. deps: whichever layers a command needs.
  tests/
    conformance/           # shared, data-driven vectors (the spec §13 backbone)
  .github/workflows/ci.yml
  docs/superpowers/specs/  # this roadmap + per-milestone specs
```

**Dependency DAG (one direction only):**

```
core ◄── cbor ◄── canonical ◄── typed ◄── compose ◄── template
  ▲                  ▲             ▲          ▲            ▲
  └──── syntax ──────┘            cli links the layers a command needs
```

**Crate creation is lazy.** Crates are created only when a milestone reaches them — no empty stubs. M0 creates the workspace root + `mangrove-core` + `mangrove-cli`; later milestones add the rest.

**Granularity note (flagged, not over-decided):** `typed`/`compose`/`template` is the most granular split. Start split (it matches the spec's subset-targeting intent and enforces the strict layering). If those three prove to churn together in practice, collapse them into one `mangrove-lang` crate. Do not pre-merge.

---

## 3. Milestone sequence

Each milestone is one spec → plan → TDD cycle. "Done" means: TDD (failing test first), and CI green (fmt, clippy `-D warnings`, build, test).

### M0 — Scaffold + CI
Workspace root, `mangrove-core` stub (error type skeleton), `mangrove-cli` stub, `.github/workflows/ci.yml`, and the **conformance harness skeleton** (a test that walks `tests/conformance/`, currently empty). Committed `Cargo.lock`. Tiny; unblocks everything.

### M1 — L0 Data (first real implementation)
Lexer → parser → `value.rs` model → canonical form (§7) → deterministic CBOR → BLAKE3 hash. Deliverable: `mangrove hash file.mang` emits the `b3:…` content address, and the conformance harness runs real `(input → canonical → hash)` vectors. This establishes the vector machinery every later layer reuses.

Covers spec §3 (lexical, primitives, scalars, strings, comments-three-fates), §7 (canonical form), §8 partial (multi-document stream parsing/addressing), §13 (vector harness for the canonical+hash vector kind).

### M2 — L1 Typed
Schema binding (§4.1), the single type grammar (§4.2), refinements + unions (§4.3), custom types (§4.4), units-as-brands (§4.5–4.6), `match` exhaustiveness (§4.8), `require` (§4.7), metadata annotations (§4.9), and **structured validation errors** (§12) with the `(input → expected-errors)` vector kind. Unit canonicalization (largest exact unit) lands here and feeds back into `mangrove-canonical`.

### M3 — L2 Composed
`use` + resolver split (document identity / `mangrove.lock` pin / `resolvers.toml` location-auth) (§5.1–5.2), spread + override deep-merge (§5.3), `@key` list ops `patch`/`append`/`remove` (§5.3), `unset` (§5.4), the decidable structural subtype check (§5.5), per-type version pinning (§5.6). Regex-subtype containment is validated at value level, not proven at type level (stated limitation §7.2/§7.3 of the RFC).

### M4 — L3 Templated
`params` as module parameters (§6.1), total non-recursive `fn` constructors (§6.2), value-layer interpolation (§6.3), `emit` document streams + `unset`-drops-document (§8), evaluation safety: pure / fuel + memory budget / opaque `secret()` (§11), emit projections + `clear()` null escape (§10), schema evolution `migrate` (§9).

---

## 4. Dependency decisions (load-bearing)

- **Arbitrary-precision numbers** — `num-bigint` for `int`, `bigdecimal` for `decimal`. IEEE-754 float never enters the value model (axiom §2.5). Decided.
- **Canonical CBOR — hand-written deterministic encoder** in `mangrove-cbor`. Off-the-shelf crates (`ciborium`, etc.) do not *guarantee* RFC 8949 §4.2 canonical bytes (sorted keys, definite lengths, shortest-form integers). Byte-exact canonicalization is the entire content-addressing thesis (§13), so the encoder is owned, ~100–200 lines, and verified against committed vectors. A decode path may use a crate later for interop; the *encoder* stays hand-written. **This is the one deliberate "do not be lazy" spot.**
- **Hash** — official `blake3` crate. Content address is `b3:<hex>` (§7).
- **Regex** (M2+) — `regex` crate for `=~` *value* validation. Regex-subtype *containment* (§5.5, PSPACE) is deferred: validated at value level, never proven at the type level, exactly as the RFC's stated limitations permit.
- **Parser** — hand-written recursive-descent (no parser-generator dependency). The grammar (spec Appendix A) is small and recursive-descent gives the precise, structured error positions §12 requires. Revisit only if the grammar outgrows it.

---

## 5. CI — one file

Single `.github/workflows/ci.yml`, one job, stable Rust toolchain, ordered:

1. `cargo fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`  *(this is the "typecheck" step — Rust has no separate one)*
3. `cargo build --workspace --locked`
4. `cargo test --workspace --locked`

`--locked` is enforced from M0 (the `Cargo.lock` is committed). Lints (`clippy -D warnings`) and shared dependency versions are configured once at the workspace root (`[workspace.lints]`, `[workspace.dependencies]`) so every crate inherits them.

---

## 6. Testing discipline (all milestones)

- **TDD**: write the failing test first, then the implementation (per the user's global workflow and `test-driven-development` skill).
- **Two vector kinds** (spec §13), both data-driven from `tests/conformance/`:
  - `(input.mang → canonical → hash)` — pins key-sort, number/unit normalization, CBOR/BLAKE3 output byte-for-byte. (M1+)
  - `(input.mang → expected-errors)` — pins validation behavior as structured data. (M2+)
- Unit tests live next to their crate; cross-layer behavior is pinned by conformance vectors so a second implementation can be checked against the same corpus.

---

## 7. Out of scope (this roadmap)

- Editor projection / conceal plugin (§14) — tooling, not the language.
- A second (Go) implementation — the conformance corpus exists to enable it, but building it is separate.
- Performance tuning beyond the §11 fuel/memory budget being present and correct.

---

## 8. Immediate next step

Proceed to a **writing-plans** cycle for **M0 (Scaffold + CI)**: the workspace skeleton, `mangrove-core` + `mangrove-cli` stubs, the CI workflow, the conformance harness skeleton, and a committed `Cargo.lock`. M1 (L0) gets its own spec + plan after M0 lands green.
