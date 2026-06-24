# M3a — L2 Local Composition: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M3a (first L2 slice; M3b = the network resolver + lockfile + per-type pins).
> **Normative source:** `mangrove-spec.md` §5.1 (local `use`), §5.3 (composition — one rule + the list exception), §5.4 (`unset`), §5.5 (subtype redefinition).
> **Goal:** Compose documents locally — `use ./base.mang`, `...spread` with last-wins deep-merge, `@key` list operations, `unset`, and subtype redefinition — with **no network**. The namespaced/remote resolver, `mangrove.lock`, and per-type pins are M3b.

Builds on L0+L1 (M0–M2c). All composition here is over local files and resolves before validation/hashing; the resolved canonical form (D12) is unchanged in spirit — composition produces a single merged value that then validates and hashes exactly as a hand-written one would.

---

## 1. Scope

**In:**
- **Local `use`** (§5.1): `use ./base.mang as base` — load a sibling file (relative path only), exposing its `type`/`unit` definitions (as `base.Name`) and its body value (for spread).
- **Spread + override** (§5.3): `...base` pastes the resolved body's fields; later statements win; records deep-merge. One rule.
- **The list exception** (§5.3): bare list assignment **replaces**; a schema list annotated `@key(field)` opts into a named `patch`/`append`/`remove` operation block; `xs += [...]` appends.
- **`unset`** (§5.4): the value meaning "absent"; removes an inherited field in an overlay; error on a schema-required field.
- **Subtype redefinition** (§5.5): `schema Base & { field: NarrowerType }` — locally narrow a type; checked `New <: Old` (structural, covariant, depth-recursive; interval/enum containment; `require` re-validated, not implication-checked; regex containment deferred per the spec's stated limit).

**Out (deferred to M3b / later):**
- Namespaced/remote `use infra/k8s/core @v5.0`, `mangrove.lock`, `.mangrove/resolvers.toml`, git fetch + hash-verify, per-type pins (§5.6) → **M3b**.
- Regex-refinement subtype containment (PSPACE) → deferred (spec limitation; `=~` narrowing is conservatively rejected unless syntactically identical).
- `match`/`params`/`emit` (L3) → M4.

---

## 2. Local `use` (§5.1)

```
use ./base.mang as base

...base                     # spread base's body
schema base.Deployment      # use a type defined in base
replicas: 6
```

- **Decision D19:** M3a supports only **relative local paths** (`./x.mang`, `../y.mang`), resolved relative to the importing file's directory. A namespaced import (`use infra/...`) is an error in M3a ("remote imports require a resolver — M3b"). Cyclic `use` (a→b→a) is a load error.
- Loading is **recursive and cached**: a `use`d file is itself parsed (its own `use`s resolved) into a resolved document. `base.Name` resolves a type/unit from base's `TypeEnv`; `...base` spreads base's resolved body.
- The importing document's `TypeEnv` is the union of its own definitions and the aliased ones (`base.X`). A name clash between a local type and `base.X` cannot occur (the alias namespaces them).

---

## 3. Composition — one rule + the list exception (§5.3)

**The rule:** statements apply in order; **later wins**; records **deep-merge**; spread is bulk assignment.

```
...base                 # paste base's fields
replicas: 6             # later statement wins (scalar override)
labels: { tier: "edge" }  # deep-merges into base.labels
```

- **Decision D20 (merge semantics):** merging two values `old`, `new`:
  - both **records/maps** → deep-merge key-by-key (recurse); keys only in one side are kept.
  - **scalar/list/kind-mismatch** → `new` replaces `old` wholesale.
  - `new == unset` → the key is **removed** (D21).
- Spread `...base` is sugar for "merge base's body into the accumulator at this point." Multiple spreads + statements compose left-to-right.
- A document with composition resolves to one merged `Value`, which then validates against `schema` and hashes (D12). Two documents that compose to the same value hash identically.

**The list exception (§5.3):**
- A bare `xs: [ … ]` **replaces** the inherited list.
- `xs += [ … ]` **appends**.
- A schema field typed `[ T ] @key(field)` enables an operation block on that list:
  ```
  containers {
    patch "api":  { image: "api:1.21.0", ports += [ 9090 ] }  # deep-merge into element key=="api"
    append:       { name: "envoy", image: "envoy:1.31" }        # add (error if key exists)
    remove:       "cron"                                          # drop element by key
  }
  ```
  Every list mutation is therefore the default (replace) or a **named, greppable verb**.
- **Decision D22:** `@key(field)` is a new field annotation (`@key` joins `@doc`/`@message`/`@deprecated`); the op block is parsed as a value form `name { patch …, append: …, remove: … }`. `patch`/`append`/`remove` are contextual keywords inside such a block, not reserved elsewhere.

---

## 4. `unset` (§5.4)

```
...base
debug_port: unset      # base had it; this document does not
```

- **Decision D21:** `unset` is a keyword **value** (`Value`-level marker `Value::Unset`, or a parse-level sentinel) legal anywhere a value is. In a merge it removes the inherited key; the result is **absence**, never a present null (axiom §2.4). After full composition, no `unset` remains in the resolved value (any leftover `unset` on a key with no inherited value simply yields absence). `unset` on a schema-**required** field is a validation error.

---

## 5. Subtype redefinition (§5.5)

```
use ./base.mang as base

schema base.Deployment & {
  replicas: int & >= 1 & <= 10     # narrow base's `int` to a subtype
}
```

- **Decision D23:** `schema Base & { … }` produces a locally-narrowed schema; the override is admissible iff `New <: Old`. The relation is **structural, covariant, depth-recursive** (spec §5.5):
  - **Scalars:** `int & P <: int & Q` iff `P ⟹ Q` — interval containment and enum/literal subset (both trivially decidable). **Regex containment is deferred** — a `=~` narrowing is accepted only if byte-identical to the original, else rejected with "regex subtype not supported (M3b/later)".
  - **Records:** every field of Old must have `New.f <: Old.f`; New's field set ⊆ Old's; required-ness may only increase (optional→required/dropped ok; required→optional forbidden); New may not add a field Old lacked. Recurse.
  - **Maps→records, lists (covariant element), unions (drop/narrow variants):** per §5.5.
  - **`require`:** New inherits Old's requires plus any new ones; all are **re-validated against values**, never implication-checked (keeps the check decidable — stated limit).
- A non-narrowing redefinition (`New </: Old`) is a load error naming the offending field and why.

---

## 6. Architecture / crates

- **New crate `mangrove-compose`** (L2 semantics): document loading (local `use`, recursive + cycle-checked), the merge/spread/unset engine, `@key` list-op application, and the `<:` subtype checker. deps: `mangrove-core`, `mangrove-syntax`, `mangrove-typed`.
- **`mangrove-syntax`:** parse `use ./x as a`, `...spread`, `unset`, `@key(field)`, the list-op block, and `schema Base & { … }`. `Value` gains an `Unset` marker (or the parser yields a compose-AST distinct from the canonical `Value`). Statement set per Appendix A grows (`spread`, `listop`).
- **`mangrove-cli`:** `hash`/`check` first **compose** (resolve `use` + spread + ops + unset → merged value) then validate/hash as today. A `--base-dir` is implicit (the file's directory).
- **`mangrove-conformance`:** L2 vectors — compose two local files → expected canonical hash; `@key` patch/append/remove; `unset`; a subtype-redefinition accept and a reject.

**Note:** composition runs **before** the resolve pass (units→base int) and validation. Pipeline becomes: parse → compose (merge local docs) → resolve (units/defaults) → validate → hash.

---

## 7. Decisions to confirm

- **D19** — local relative `use` only in M3a; remote/namespaced → M3b; cyclic use is a load error.
- **D20** — merge: records/maps deep-merge, everything else replaces; later wins.
- **D21** — `unset` is a value marker that removes a key on merge → absence; `unset` on a required field errors.
- **D22** — `@key(field)` annotation + `patch`/`append`/`remove` op block + `+=`; bare list assignment replaces.
- **D23** — `schema Base & {…}` subtype redefinition, `New <: Old` structural/covariant/recursive; interval/enum containment; regex containment deferred; `require` re-validated not implied.

**Implementation order within M3a:** (1) local `use` + spread + deep-merge + `unset` [the composition core]; (2) `@key` list ops + `+=`; (3) subtype redefinition. Each its own TDD cycle and commit.
