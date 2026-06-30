# Changelog

## v0.10.1

Two refinements to the v0.10.0 work.

### Fixed
- **`import --skip-empty` yields `[]` for an all-empty stream.** When every
  document in a stream is blank (e.g. a `helm template` where all resources are
  disabled), `--skip-empty` now produces an empty list — which validates against
  `[Resource]` — instead of an "empty document" error. Without the flag, an
  empty stream still errors.
- **Front-end parity on malformed list input.** The lossless CST was more lenient
  than the evaluation parser for some space-/newline-separated list elements
  (e.g. `[ 1 2 ]`, or an element and an `if` split across a newline): the parser
  errored while the CST silently accepted. Both front-ends now reject these
  identically, restoring the parse-equivalence invariant.

## v0.10.0

List composition and conditional resources — so a document that evaluates to a
list of resources (e.g. a Kubernetes manifest set) can include items
conditionally and splice in shared lists, without leaving the value model (no
null, `unset` stays binding-level). Both new forms are first-class across the
evaluation parser and the lossless CST, kept in agreement by the
parse-equivalence + content-hash oracle.

### Language
- **List spread.** `[ 0, ...xs, 3 ]` splices the elements of a list-valued
  expression into a list literal — the list analogue of map spread (`...alias`).
  Resolved at evaluation, so `[ 0, ...[1, 2], 3 ]` is exactly `[ 0, 1, 2, 3 ]`
  and hashes identically. Spreading a non-list is a clean error.
- **Conditional list elements.** `[ namespace, alertSink if cfg.enabled ]`
  includes an element when the condition is `true` and omits it when `false`.
  Sugar for `...match cond { true: [item], false: [] }`; the item may be any
  value (a whole resource record included). A non-bool condition is a clean
  error, never a silent omission.
- **`match` over `bool` is total without `_`.** A match whose arms cover both
  `true` and `false` is now exhaustive for any scrutinee (previously a `_` arm
  was required unless the scrutinee was a statically-bool-typed name). A
  non-bool scrutinee still errors as uncovered.

### Interop
- **`import --skip-empty`.** Drops blank/`null` documents from a multi-document
  YAML stream (e.g. `helm template` emits a blank document per disabled
  resource) instead of rejecting the stream. The accepted flag works before or
  after the path. This does not weaken the no-null axiom: an empty *document* in
  a stream is "no document", not a null *value* — a null value inside a document
  is still rejected.

## v0.9.2

`gen-openapi` upgrades that let a whole multi-resource Kubernetes repo be
typed from one command. Verified end-to-end: every k8s/CRD manifest in a real
GitOps repo validates against types generated in a single invocation. Additive
— existing single-spec/single-root behaviour is byte-identical.

### Interop — `gen-openapi`
- **Combine multiple schemas/roots in one run.** `gen-openapi [--k8s] <spec>…
  [--root <Def>]…` accepts several schema files (or several roots within one
  envelope) and emits a single deduplicated type set with exactly one `Json`
  free-form type. Previously each invocation re-emitted `type Json`, so
  concatenating outputs for a multi-kind schema failed with
  `duplicate type definition: Json`.
- **`--k8s` injects the standard resource envelope.** Many CRD JSON schemas
  describe only `spec`/`status`; the generated closed record then rejected a
  real manifest's `apiVersion`/`kind`/`metadata` as unknown fields. With
  `--k8s`, those three fields are injected into each root object type when
  absent (`kind` as a literal of the `--root` name, so it also serves as a
  discriminant) — never overwriting fields the schema already declares.

## v0.9.1

Two additive refinements that make typing real Kubernetes manifests clean
(surfaced migrating a GitOps repo). Neither changes any existing accept/reject
behaviour.

### Typed
- **Discriminated-union dispatch fires on an optional discriminant.** Detection
  no longer requires the discriminant field (e.g. `kind`) to be non-optional —
  it must still be present in every variant, literal-typed, and pairwise
  distinct. This matters because `gen-openapi` emits k8s discriminants as
  optional (`kind?: "PersistentVolumeClaim"`), so precise per-resource errors
  (`[0].spec.accessModes: got 123, expected [ str ]`) now actually fire on
  generated types instead of degrading to "no matching variant". A value that
  omits the optional discriminant still falls back to try-each.

### Interop
- **`gen-openapi` accepts a bare-root JSON Schema.** A standalone JSON Schema
  file (top-level `properties`/`type`, no `definitions`/`components.schemas` —
  the common per-resource k8s/CRD schema shape) is now generated directly,
  named by `--root`, with no OpenAPI-envelope wrapping step.

## v0.9.0

Multi-document interop. A multi-document YAML stream (e.g. a Kubernetes
manifest bundle) now imports to a single Mangrove document whose body is a
list of resources, round-trips losslessly back to a `---`-separated stream,
and validates per-resource with precise errors. This required two language
additions, surfaced by migrating a real GitOps repo.

### Language
- **Bare-value top-level documents.** A document body may be a single value —
  a list, scalar, string, `match`, or reference — not only `key: value`
  bindings. `[ 1, 2, 3 ]` is now a complete document (it reduces to a list,
  the same as any other value). Implemented in both the evaluation parser and
  the lossless CST, kept in agreement by the existing parse-equivalence and
  content-hash oracle. (`{`-led bodies remain bindings.)
- **Discriminated-union validation.** When a union's variants are records
  sharing a common, required, pairwise-distinct literal field (e.g. `kind`),
  validation dispatches on that field to the matching variant and reports
  precise per-field errors (`[2].spec.storage: got 12, expected str`) instead
  of a generic "no matching variant". An unknown discriminant lists the valid
  values. Unions without such a field fall back to the prior try-each-variant
  behaviour; the accept/reject set is unchanged — only error precision improves.

### Interop
- **Multi-document YAML.** `mangrove import` turns a multi-doc YAML stream into
  a `Value::List` (a single doc stays a scalar/map as before); `mangrove export
  --to yaml-stream` emits a list body as `---`-separated YAML documents. The
  round-trip is content-hash-stable.

### Fixed
- **Bare-value bodies no longer collapse to `{}`.** Composition rebuilt the body
  by folding binding statements into an empty map and ignored a bare-value body,
  so every bare-value document (and an empty file) evaluated to the same empty
  map. Composition now uses the body directly when there are no statements.
- **`unset` reaching a final value is a clean error, not a crash.** A bare
  `unset` document, or an `unset` surviving inside a list (`[ 1, unset, 3 ]`),
  previously aborted the process at the encoder. It is now rejected with a
  message; `unset` remains valid only where it removes a binding during
  composition.
- **Spreading a non-record document is an error.** `...alias` where the spread
  source's body is a list/scalar (now possible with bare-value bodies) is
  rejected instead of silently discarding the data.

## v0.8.0

Editor/tooling round: a Tree-sitter grammar, smarter completion, and
workspace-wide navigation. The LSP stays read-only and network-free — all
cross-file/workspace work reads local files (resolved via the committed
lockfile) and returns a `WorkspaceEdit` for the client to apply; nothing is
fetched or written.

### Editors
- **Tree-sitter grammar** (`tree-sitter-mangrove/`) covering the full surface
  syntax, with highlight queries. Gives `.mang` files immediate syntax
  highlighting (in Neovim/Zed/Helix/…) before the LSP attaches, independent of
  the server's semantic tokens.

### Tooling — `mangrove lsp`
- **Enum-value completion.** In a field's value position, completion offers the
  field type's literal-union values (e.g. a `gen-openapi` enum), resolving the
  type locally, through a named alias, or from an imported package.
- **Workspace-wide find-references and rename.** Both now span every `.mang`
  file under the workspace root: a file is matched only when its `use` alias
  resolves (by canonical path) to the symbol's defining file, so unrelated
  same-named symbols are never touched. Rename produces a multi-file
  `WorkspaceEdit`; renaming symbols inside external/imported packages is declined.

### Fixed / internal
- Skip symlinked directories in the workspace walk (no cycles); allow `-` in
  rename target identifiers (matching the lexer); documented duplicate-`use`-alias
  rejection and the import-cache invalidation contract.

## v0.7.1

Robustness fix from a whole-arc adversarial review.

### Fixed
- **No stack overflow on deeply nested values.** The CST's value parser recursed
  without a depth limit, so a deeply nested input (e.g. `x: ` followed by tens of
  thousands of `[`) overflowed the stack and aborted the process — uncatchable, so
  it would crash `mangrove fmt` and the LSP server. Value nesting is now bounded
  (depth 128, matching the evaluator's limit); past the cap the remainder is
  consumed into an error node, keeping the tree lossless. Normal documents are
  unaffected.

### Docs
- README status line corrected to the current version; fixed the Zed
  `brackets` config (it must be an array of bracket pairs, not a boolean).

## v0.7.0

LSP completion polish, an import-read cache, and a Zed editor extension. Still
read-only and network-free — imported files are read from disk (resolved via the
committed lockfile); git-backed namespaces are never fetched.

### Tooling — `mangrove lsp`
- **Imported-schema field completion.** A document bound to an imported record
  schema (`schema alias.Record`) now gets that record's field-name completions
  (read read-only from the resolved file). Also fixes a bug where a `use` decl
  disabled field completion for a *local* schema.
- **Alias-prefix completion filtering.** After typing `alias.`, completion offers
  only that package's types (as bare names); without a prefix it no longer dumps
  the entire imported type set — so a large `gen-openapi` import stays usable.
- **Import-read cache.** Imported files are cached (keyed by mtime + length), so
  cross-file go-to-definition and imported-type completion don't re-read and
  re-parse from disk on every keystroke; a changed file is re-read automatically.

### Editors
- **Zed extension** (`editors/zed/`) registering the `Mangrove` language and
  wiring up `mangrove lsp`. Install as a Zed dev extension (see `editors/README.md`).

## v0.6.0

LSP cross-file awareness: navigate into and complete against imported types
(including those generated by `mangrove gen-openapi`). Still strictly read-only
and network-free — imported files are resolved via the committed lockfile and
read from disk; git-backed namespaces are never fetched.

### Tooling — `mangrove lsp`
- **Cross-file go-to-definition now works in value position and on
  partially-edited documents.** Import aliases are resolved straight from the CST
  rather than requiring the whole document to evaluate, so navigation works while
  you type and for qualified references anywhere.
- **Completion offers imported types.** In type position, `use`d packages
  contribute `alias.TypeName` completions (read read-only from the resolved local
  file) — so types generated with `gen-openapi` and imported are completable.

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
