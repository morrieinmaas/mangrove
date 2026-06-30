# Mangrove

A typed, composable, content-addressed configuration language — implemented in Rust.

Mangrove is built in layers: plain **data** (L0), a **type system** (L1), **composition** (L2),
and **templating** (L3), on top of a supply-chain layer for verified imports. Every document
reduces to a single canonical value with a stable BLAKE3 content hash, so two documents that
*mean* the same thing hash the same.

> **Status:** v0.11.0 — an experimental, solo, spec-complete implementation with a
> formatter and a language server. Not used in production yet. The ideas (below) are
> the point; a hosted docs site and broad editor packaging are still to come.

## Why

Configuration today is mostly untyped text that tools interpret by convention. The pain is familiar:

- **YAML** has no types and a surprising value grammar (`no` → `false`, sexagesimal, `null` everywhere). A typo is a silent wrong value, not an error.
- **Helm / Kustomize** template *text*, so a mis-indented value silently corrupts structure, and "is this the same config?" has no answer.
- **Drift & trust:** two configs that produce the same result can look different (and vice-versa), and an imported chart/base can change underneath you with nothing to detect it.

Mangrove takes three positions in response:

1. **The schema is the only type authority — no inference.** There is exactly one canonical form for a value, so equality is decidable and a diff is meaningful.
2. **Config is a value, addressed by content.** Every document reduces to a deterministic CBOR encoding hashed to `b3:<hex>`. Two documents are "the same config" iff they hash the same — across composition, templating, and format conversion.
3. **Imports are supply-chain-verified.** A namespaced import is pinned in a committed `mangrove.lock` and hash-verified before evaluation, fail-closed; each package anchors its own lock. Templating operates on **values, not text**, so it can never corrupt structure.

## How it compares

| | typed | one canonical form | content hash | **verified imports / lockfile** | templating |
|---|:--:|:--:|:--:|:--:|:--:|
| YAML / Helm | ✗ | ✗ | ✗ | ✗ | text |
| Jsonnet | ✗ | ✗ | ✗ | ✗ | values |
| Dhall | ✓ | ✓ | ✓ (semantic) | imports hashed, no lockfile | values |
| CUE | ✓ (lattice/inference) | ✗ | ✗ | ✗ | unify |
| Nickel | ✓ (gradual) | ✗ | ✗ | ✗ | values |
| Pkl / KCL | ✓ | ✗ | ✗ | ✗ | values |
| **Mangrove** | ✓ (no inference) | **✓** | **✓ (BLAKE3)** | **✓ committed lock, fail-closed, per-package** | values |

The closest neighbour is **Dhall** (typed, total, semantic hashing). Mangrove differs in two deliberate ways: **no type inference** (the schema is the sole authority — one canonical form, simpler errors, no lattice to reason about), and a **package-manager-style supply chain** (a committed lockfile, hash-verify-before-eval that fails closed, local + git backends, per-package anchoring) rather than per-import frozen hashes only. The supply-chain integrity story is the part no other config language really leans into — and it's the reason Mangrove exists.

See the [language specification](mangrove-spec.md) and the [design RFC](mangrove-rfc.md) for the full rationale.

## Design axioms

- **No surface type inference** — the schema is the sole type authority, enabling one canonical form.
- **No null** — absence is expressed by composition (`unset`), never a null value.
- **Arbitrary precision** — integers and decimals are exact (`BigInt`/`BigDecimal`); no IEEE float.
- **Content-addressed** — the canonical form is deterministic CBOR (RFC 8949 §4.2/§7.1) → `b3:<hex>`.
- **Verify before eval** — namespaced imports are hash-verified against a committed lock, fail-closed.

## The layers

| Layer | What it adds |
|-------|--------------|
| **L0 — Data** | maps, lists, strings (incl. text blocks & raw), ints, decimals, bools, bytes |
| **L1 — Typed** | `type`/`schema`, refinements (`int & >= 1 & <= N`, `str & =~ regex & len >= 1 & len <= 63`), unions (incl. `kind`-discriminated, for precise per-variant errors), literals, units (`512Mi`), brands, `require`, annotations (`@key`, `@message`, `@deprecated`), defaults, **productive recursive types** (arbitrary-JSON / trees) |
| **L2 — Composed** | `use` + spread (`...alias`), deep merge, `unset`, `@key` list-ops, subtype redefinition (`schema Base & {…}`) |
| **L3 — Templated** | `params`, references, string interpolation (`${v}`), `match`, schema `fn` constructors, module calls (`emit: webapp(...)`) |
| **Supply chain** | resolver split (identity/location/auth), local + git backends, `mangrove.lock` hash-verify, per-package anchoring, cross-file type imports, per-type version pins |
| **Interop** | YAML/TOML ⇄ Mangrove converters (`import`/`export`), incl. multi-document YAML streams (`--to yaml-stream`); `gen-openapi` types from an OpenAPI/k8s API spec |

## CLI

```
mangrove hash   <file.mang>            # the BLAKE3 content address of the canonical value
mangrove check  <file.mang>            # validate against the bound schema
mangrove fmt    <file.mang>            # format in place (--check for CI, - for stdin)
mangrove update <file.mang>            # resolve + pin namespaced imports into mangrove.lock
mangrove import <file.yaml|.toml>      # convert YAML/TOML to a schemaless Mangrove document
mangrove export <file.mang> --to yaml  # evaluate and emit YAML/TOML (--to yaml-stream for a multi-doc list)
mangrove gen-openapi <spec.json> --root <Def>   # OpenAPI (e.g. the k8s API) → Mangrove types
mangrove lsp                           # run the language server over stdio (for editors)
```

## Editor support

`mangrove lsp` is a read-only, network-free [language server](editors/README.md):
diagnostics (parse + schema errors), hover, document symbols, semantic-token
highlighting, formatting, context-aware completion (including imported/`gen-openapi`
types and literal-union/enum values), go-to-definition (local and cross-file into
imported types), and workspace-wide find-references and rename. Neovim
([`editors/nvim/`](editors/nvim/)) and Zed ([`editors/zed/`](editors/zed/)) setups are provided;
any LSP client can launch `mangrove lsp`. A [Tree-sitter grammar](tree-sitter-mangrove/)
gives immediate syntax highlighting on open (before the LSP attaches); the LSP's
semantic tokens add type/reference-aware highlighting once it connects.

## Kubernetes

Mangrove works as the authoring layer for Kubernetes manifests — write typed,
content-addressed `.mang`, evaluate to YAML, feed it to the cluster. A
`kubectl-mangrove` plugin, a Kustomize/kpt KRM function, and a container image
are in [`k8s/`](k8s/); generate Mangrove types for the real k8s API with
`mangrove gen-openapi` (point it at `kubectl get --raw /openapi/v2`). See
[`k8s/README.md`](k8s/README.md).

## Example

```
type Server = { host: str, port: int & >= 1 & <= 65535 }
schema Server

host: "api.example.eu"
port: 8443
```

See [`examples/`](examples/) for a Kubernetes Deployment (with units, refinements, `@key` lists),
a templated per-environment Deployment (`params` + `match` + interpolation), and a `pyproject`.

## Migrating an existing project

Moving a project's Kubernetes YAML, Helm charts, or pyinfra/YAML configs onto
Mangrove is incremental and hash-verified at every step (equal `b3:` hashes ⇒
provably the same config). See [`docs/MIGRATING.md`](docs/MIGRATING.md) for the
phased path and [`docs/migrate.just`](docs/migrate.just) for drop-in `just`
recipes (`import-all`, `check`, `fmt-check`, `render`, `verify`).

## Building

Rust 2024 edition (≥ 1.85). The build is gated by `just ci` (fmt-check → clippy `-D warnings`
→ build → test). `unsafe` code is forbidden workspace-wide.

```
just ci      # the full gate CI runs
just test    # tests only
```

## License

Apache-2.0.
