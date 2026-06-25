# M6 — Cross-file type imports + per-type version pins (§5.6)

**Goal:** let a document reference **type definitions** from a `use`d module — `schema k.Deployment`, `field: k.Probe` — and pin an individual type to a different version of its package (`k.Probe @v4.8`). Until now `use` imported a module's *value* (for spread/module-call); M6 also imports its *types*.

## Slices

### M6a — cross-file type imports

- **D48 — qualified type names.** A `use`d alias exposes its module's `type`/`unit` definitions under qualified names `alias.Name`. They are usable wherever a type is: `schema alias.Type`, a field type `f: alias.Type`, inside unions/lists/brands. No new value semantics — only the type namespace grows.
- **Resolution & self-consistency.** An imported type's *internal* references (e.g. `Probe` mentions `Handler`, both defined in the module) must resolve within the module, not the importer. So when importing a module's types, each is registered under `alias.Name` with its inner `Named("X")`/unit references **rewritten** to `alias.X` for every name the module defines (built-ins `int`/`str`/… are left alone). The importer's `TypeEnv` then holds a self-consistent set of `alias.*` entries alongside the root's own types; `resolve("alias.Type")` works transitively. Name clashes can't occur (the `alias.` prefix namespaces them); a `use` alias collision is already an error (M4d.2).
- **Parsing.** The type grammar accepts `Ident.Ident` as a qualified `Named("alias.Name")` (the lexer already emits `Dot`). `schema` accepts `Name` or `alias.Name`.
- **Architecture.** `compose` already keeps each alias's full sub-`Composed` (its `typedefs`/`unitdefs`). A new step (in the CLI's `evaluate`/`cmd_check`, where the root `TypeEnv` is built, and in `build_modules`) folds the imported, namespace-rewritten type/unit defs into the env used for validate/resolve. `TypeEnv::build` gains a way to register extra qualified defs (e.g. `TypeEnv::build_with_imports(root_types, root_units, imports)` where `imports: &[(alias, &Composed)]`).
- **Scope (M6a):** types are imported from the **same version** the alias is `use`d at (one version per alias). Multi-version (the pin) is M6b. The imported module is resolved/verified by the existing compose path, so type imports inherit hash-verification (D27) for free.

### M6b — per-type version pins (§5.6)

- **D49 — a pin is a type-position version override.** Syntax: a qualified type reference may carry `@<ref>` — `f: k.Probe @v4.8`. It means "validate/resolve this slot against version `<ref>` of the package behind `k`'s namespace, its `Probe` type", overriding the version `k` was imported at. It is an **override insertion, not a skew check** (§5.6): the pinned type simply governs that subtree; any resulting mismatch with a parent surfaces as an ordinary validation error.
- **Mechanism.** Resolving `k.Probe @v4.8` fetches the package behind `k`'s namespace at ref `v4.8` (via the existing resolver+lock+verify path — a second checkout of the same repo at a different ref), takes its `Probe` type (namespace-rewritten as in M6a), and uses it for that field. The lockfile records a distinct entry `"<ns>@v4.8#Probe" = "b3:<source-hash>"` for reproducibility (the *intent* lives in the document; the lock holds the resolved hash). `mangrove update` walks pinned references too.
- **Canonical form.** The hash is still of the resolved value. A pin changes the hash only insofar as it changes the resolved value (e.g. v4.8's defaults/units differ from v5.0's) — which is correct: different meaning → different value → different hash; identical resolution → identical hash. No artificial hash perturbation.
- **Staleness lint (advisory).** A pin older than the alias's main version is flagged as tech-debt (a warning, never an error), per §5.6.
- **Scope (M6b):** the pin's `<ref>` must be a ref the resolver can fetch (a git tag / local dir as in M3b). Pins are honored in field-type position in `type` definitions and in a document's effective schema; a full document-body path pin (`container.probe: k.Probe @v4.8` as a body statement) is **out** unless it falls out naturally — the type-position form captures §5.6's intent without adding value-position type annotations (which would reintroduce inference, §2).

## Testing

Hermetic, golden-hash anchored. M6a: a module defining types; a root `use`ing it and binding `schema k.T` / `f: k.T`; valid + invalid documents; an imported type whose inner refs resolve; a hash equal to the same types written inline. M6b: two local "versions" of a package (different dirs/refs), a field pinned to the older one, asserting the pinned type governs (a value valid under v4.8 but not v5.0 passes only when pinned); the lock gains the `#Type` entry; `update` writes it; the staleness warning fires.

## Out of scope

- Value-position type annotations (no inference, §2).
- Importing `fn`s across files (M6 imports types; cross-file `fn` is a later, separate concern).

---

## Post-review notes (M6)

The M6 adversarial review came back clean on all four highest-severity guarantees
(integrity-through-pins, canonical-form/type soundness, pin-encoding robustness,
depth/termination). Three non-blocking notes; dispositions:

- **Pins in `params`/`fn` type positions are now fetched.** `collect_pin_refs` also
  scans param types and fn param/return types, so a pin written there resolves
  end-to-end (previously it would have been unresolvable).
- **Staleness lint (advisory "pin older than the alias's main version") is dropped,
  not deferred.** Versions are opaque resolver refs (git tags / dir names); "older"
  is not generally computable without imposing a version ordering the language does
  not define. If a project wants this, it belongs in a linter with a project-specific
  version scheme, not the core.
- **Cross-file types inside a *called* module** remain one-level/deferred (M6 imports
  types into the root env). Such a type is rejected (fail-closed), only the error
  wording is generic — a known, harmless UX rough edge.
