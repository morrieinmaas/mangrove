# Changelog

## v0.2.0

Cross-file types, recursive types, and Kubernetes interop — each milestone
test-first and adversarially reviewed.

### Language
- **Cross-file type imports (§5.6):** a `use`d module's types are referenceable
  as `schema k.Deployment` / `field: k.Probe` (imported types' internal
  references are namespace-rewritten so they resolve self-consistently).
- **Per-type version pins (§5.6):** `k.Probe @"v1"` validates a slot against a
  different fetched version of the package, recorded in the lockfile.
- **Productive recursive types:** recursion is allowed when guarded by a
  record/list/map (it terminates on a finite value), so arbitrary JSON
  (`Json = str | int | decimal | bool | [Json] | { [str]: Json }`) and
  self-recursive schemas are expressible. Non-productive cycles (`T = T`) are
  rejected; `fn`/evaluation stays non-recursive. Validation is depth-guarded.
- **Text-block interpolation:** `"""…${v}…"""` interpolates (raw `r"""` opts out).

### Supply chain
- **Per-package resolver/lock anchoring:** each package resolves and verifies its
  own dependencies against its own `.mangrove/` + lock (supersedes the global
  model); fail-closed at every level, fully backward-compatible.

### Kubernetes & interop
- **`mangrove gen-openapi`:** generate Mangrove types from an OpenAPI v2/v3 spec
  (the k8s API). Free-form objects → the recursive `Json` type; recursive schemas
  emitted faithfully; closure-from-`--root`.
- **k8s tooling:** a `kubectl-mangrove` plugin (`render`/`apply`/`diff`), a
  Kustomize/kpt KRM function, and a container image (see `k8s/`).

### Quality
- Test coverage raised to ~90% (line); doctests on every library crate.

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
