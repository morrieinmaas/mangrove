# Changelog

## v0.1.0

First release: the complete Mangrove language (spec §1–§6) plus the supply-chain
and interop layers. Every milestone was developed test-first and passed an
adversarial security/correctness review.

### Language

- **L0 — Data:** maps, lists, strings (plain, raw, text blocks), arbitrary-precision
  ints and decimals, bools, bytes. Canonical form = deterministic CBOR + BLAKE3 (`b3:`).
- **L1 — Typed:** `type`/`schema`; refinements (`int & >= 1 & <= N`, regex); unions;
  literal types; units (`512Mi`, declared suffix sets → base int); brands; `require`
  predicates; annotations (`@key`, `@message`, `@deprecated`); field defaults.
- **L2 — Composed:** `use` + spread, deep merge (later-wins), `unset`, `@key` list-ops
  (patch/append/remove), subtype redefinition (`schema Base & {…}`, checked covariant).
- **L3 — Templated:** `params` (default = optional, none = required); bare-name
  references; string interpolation in plain and text-block strings (`${v}`, raw opts
  out); `match` (total — exhaustive or `_`); schema-defined `fn` constructors; module
  calls (`emit: webapp(env: "prod")`), including nested calls and unit resolution.

### Supply chain

- Resolver split: identity (`use "ns/x@v1"`), location (`.mangrove/resolvers.toml`),
  pin (committed `mangrove.lock`). Local-directory and **git** backends.
- Verify-before-eval, fail-closed (read-once, no TOCTOU; `ext::` RCE blocked; arg/path
  injection validated).
- `mangrove update` writes the lock. Per-package resolver/lock anchoring: a dependency
  resolves and verifies its own deps against its own committed config.
- Cross-file type imports (`schema k.Deployment`, `field: k.Probe`) and per-type
  version pins (`k.Probe @"v1"`).

### Interop

- YAML/TOML ⇄ Mangrove converters (`import`/`export`). Exact numbers (no f64 for YAML),
  no null (rejected), round-trips preserve the content hash.

### Tooling

- CLI: `hash`, `check`, `update`, `import`, `export`.
- Rust 2024, `unsafe` forbidden, every recursive pass depth-bounded; CI gates on
  `just ci` (fmt-check → clippy `-D warnings` → build → test).
