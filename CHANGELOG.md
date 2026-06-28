# Changelog

## v0.5.2

LSP polish from the whole-arc review.

### Fixed
- **Cross-file go-to-definition on paths with spaces/unicode.** The LSP now
  percent-decodes `file://` URIs (and handles the Windows `file:///C:/…` drive
  form), so navigating into imported types works for documents under paths like
  `…/Application Support/…`. Previously the wrong path was computed and the jump
  silently did nothing.

### Internal
- Added an LSP-level test pinning the no-fetch contract (cross-file goto into a
  git-backed namespace returns nothing, never fetches), targeted formatter
  spacing tests for `@`/`?`/`!`/`...`, and removed dead match arms for
  reserved-but-unemitted syntax kinds.

## v0.5.1

Robustness fixes for the CST front end (and therefore `mangrove fmt` and the
LSP), found by an adversarial whole-arc review.

### Fixed
- **No panic on non-ASCII tokens.** The lossless lexer's error-recovery fallback
  advanced one byte at a time, slicing multi-byte UTF-8 (e.g. a stray `é` or an
  emoji in token position) at a non-char boundary and panicking. It now advances a
  whole codepoint. `mangrove fmt` on such a file formats cleanly instead of crashing.
- **No infinite loop on mismatched closers.** A foreign closer inside a container
  (e.g. `x: [ } ]`) made error recovery break while consuming zero tokens, spinning
  forever. Records and lists now consume a foreign closer into an error node,
  guaranteeing progress; the tree stays lossless.

### Internal
- Added a corpus-wide assertion that the CST's per-token `SyntaxKind` matches the
  legacy lexer (closes an equivalence-oracle blind spot, since the LSP branches on
  token kinds). Dropped an unused `rowan` dependency from `mangrove-fmt`.

## v0.5.0

LSP feature round: completion gets smarter and the server gains navigation and
refactoring. All additions are pure CST/Document analysis — the read-only,
no-network invariant still holds (cross-file navigation only *reads* local files
resolved via the committed lockfile; it never fetches).

### Tooling — `mangrove lsp`
- **Context-aware completion:** offers items by cursor position — type/unit names
  in type position, declaration keywords at top level, the bound schema's record
  fields inside a record, value keywords in value position (with a non-empty union
  fallback when context is ambiguous).
- **Find-references** and **rename** (local / same-file): both confined to the
  symbol under the cursor — type/unit names vs value bindings/params are kept
  disjoint by CST context, so a rename never touches a same-named record field key.
  Rename returns a `WorkspaceEdit`; the server writes nothing.
- **Cross-file go-to-definition:** jumps into an imported package's type/unit
  declaration (`alias.Type`) by resolving the import to a local file via the
  committed lockfile (read-only) — returns nothing rather than fetching when the
  package isn't present locally; git-backed namespaces are never fetched.

## v0.4.0

The Mangrove language server — the last of the v0.3.x tooling series.

### Tooling
- **`mangrove lsp`:** a read-only, network-free language server over stdio
  (`lsp-server`, sync), built on the lossless CST and the existing type
  pipeline. Features: parse + schema **diagnostics**, **hover** (the declaration
  under the cursor plus its `##` doc comment), **document symbols** (outline of
  types / units / schema / params / fns / bindings), **semantic-token
  highlighting** (no tree-sitter grammar — classification comes straight from the
  CST), **formatting** (delegates to `mangrove fmt`), **go-to-definition** (local:
  references, type/unit declarations, schema names), and **completion** (declared
  type/unit names, keywords, and the bound schema's record fields). Full reparse on
  change; in-memory document store. UTF-16 position encoding (CRLF-aware), with
  precise diagnostic spans located in the CST; hardened by an adversarial review
  (panic-isolated request handling, correct request ids on malformed input).
- **Read-only invariant:** documents that `use` namespaced imports skip the
  type-check stage; the server never resolves imports, fetches, or writes files.
- **Editor integration:** a Neovim plugin (`editors/nvim/` — filetype detection
  + native `vim.lsp` setup) and `editors/README.md`.

## v0.3.0

A lossless CST front end and the `mangrove fmt` formatter built on it, plus a
migration guide. Also sets the workspace version to match the release (it had
lagged at 0.1.0 through the prior tags). LSP support is the next step in the
v0.3.x series.

### Syntax / front end
- **Lossless CST:** a concrete syntax tree (rowan-based) that preserves every
  byte — whitespace, comments, and recovers from parse errors into a complete
  tree. Evaluation keeps the fast legacy parser; tooling (fmt, future LSP) reads
  the CST. The two front ends are proven equivalent over the example corpus.

### Tooling
- **`mangrove fmt`:** a deterministic, comment- and meaning-preserving formatter
  (normalizes inline spacing and 2-space depth indentation, collapses blank
  runs, drops trailing commas) wired into the CLI: `fmt <file>…` rewrites in
  place, `fmt --check <file>…` exits 1 if any file would change (writes nothing,
  for CI gates), `fmt -` formats stdin → stdout. Built on the lossless CST, so
  it produces best-effort output even on parse errors.

### Docs
- **Migration guide** (`docs/MIGRATING.md` + `docs/migrate.just`): a phased,
  hash-verified path for moving Kubernetes/Helm/pyinfra YAML onto Mangrove, with
  drop-in `just` recipes.

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
