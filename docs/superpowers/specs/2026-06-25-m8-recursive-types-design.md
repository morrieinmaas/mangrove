# M8 — Productive recursive types (relaxing the no-recursion axiom for *types*)

**Goal:** allow recursive `type` definitions so Mangrove can represent the things
it currently can't — arbitrary JSON (a recursive `Json` union) and self-recursive
schemas (k8s CRD `JSONSchemaProps`) — *without* weakening termination or the
content-addressing guarantee. Recursion stays banned exactly where it matters:
**`fn`/evaluation** (§6.2), where it would threaten termination of *evaluation*.

## The insight

Recursion is dangerous for **evaluation** termination — that's why a `fn` may not
recurse. But a recursive *type* validating a **finite value** always terminates,
provided the recursion is *productive*: every cycle passes through a
value-consuming constructor, so each recursive step inspects a strictly-smaller
part of the (finite) value. Arbitrary JSON's natural type *is* recursive
(`Json = str | int | decimal | bool | [Json] | { [str]: Json }`), and it is
productive — so this is the right, precise representation, not a hack.

## Decisions

- **D51 — productive recursion is allowed; non-productive recursion is rejected.**
  A type reference is **guarded** if it occurs inside a value-consuming
  constructor — a record *field type*, a *list element* type, or a *map value*
  type. It is **unguarded** if reachable without crossing one — a `Union` variant,
  a `Brand` inner, or a direct `Named` alias (`type A = B`). A type's recursion is
  **productive** iff the *unguarded* reference graph is acyclic (every cycle in the
  full graph passes through ≥1 guarded position). `TypeEnv::build` accepts
  productive recursion and still rejects non-productive cycles:
  - rejected: `type T = T`, `type T = T | int`, `type A = B; type B = A`,
    `type T = brand T` — these would loop with no value consumed, and denote nothing.
  - accepted: `type Json = str | … | [Json] | { [str]: Json }`,
    `type Tree = { value: int, children: [Tree] }` — cycles go through a list/map/field.
- **D52 — validation is depth-bounded.** `check` gains a depth counter and errors
  ("type nesting too deep") past a high bound. Productive recursion on a finite
  value terminates on its own (the value bottoms out, and input depth is already
  parser-bounded at 128); the guard is defense-in-depth so no future bug can loop.
  The unknown-reference check still runs over *all* refs; only the *cycle* check
  uses the unguarded graph.
- **D53 — recursive types are not allowed in `schema Base & {…}` narrowing.**
  Subtyping compares *type against type* (no value to shrink), so a recursive type
  there would loop. We do **not** attempt coinductive subtyping; the existing
  `MAX_DEPTH` guard in `sub()` already rejects it (fails closed). A recursive type
  used as a plain schema/field type is fine — only the narrowing position is barred.
- **D54 — the OpenAPI generator exploits recursion.** With productive recursion
  available, the generator emits a built-in
  `type Json = str | int | decimal | bool | [Json] | { [str]: Json }` and maps a
  free-form object (`additionalProperties: true` / untyped object) to `Json`
  instead of the lossy `OpaqueObject`; and it emits self-recursive schemas
  faithfully (no cycle-break) when the recursion is productive. A genuinely
  non-productive cycle (rare in OpenAPI) still degrades to opaque + a warning.
  **Honest caveat:** `Json` has no `null` (Mangrove axiom §2.4), so a free-form
  value containing an explicit JSON `null` is rejected — consistent with the
  language's no-null stance.

## Canonical form is unaffected

Values are finite trees; recursion lives only in the *type*, used for validation.
The CBOR encoding and `b3:` hash are computed from the value exactly as before — a
recursive type changes nothing about a value's canonical form. (`resolve` likewise
recurses on the value, not the type graph.)

## Architecture

- `crates/mangrove-typed/src/env.rs`: keep `collect_refs` (all refs) for the
  unknown-reference check; add `collect_unguarded_refs` (refs not under a
  record/list/map) and run the existing iterative three-colour cycle DFS over
  *that* graph. A cycle there → "non-productive recursive type" error.
- `crates/mangrove-typed/src/validate.rs`: thread a `depth` through `check`
  (and `check_unit`), erroring past `MAX_DEPTH`.
- `crates/mangrove-compose/src/subtype.rs`: no change needed (the depth guard
  already rejects recursive narrowing); optionally improve the error wording.
- `crates/mangrove-openapi/src/lib.rs`: add the `Json` emission + map free-form →
  `Json`; only opaque-break *non-productive* cycles.

## Testing (TDD + e2e)

- env unit: `type T = T`, `T = T | int`, mutual `A=B;B=A`, `brand T` all rejected
  ("non-productive"); `Json` union, `Tree` record, mutual-through-list all accepted.
- validate: a recursive `Json`/`Tree` validates a nested finite value; a value
  deeper than the bound errors cleanly (no overflow); a non-map/list value against
  `Json` resolves to a scalar variant.
- subtype: a recursive type in `schema Base & {…}` errors (fails closed).
- canonical: a value typed by a recursive schema hashes identically to the same
  value typed by an equivalent inline/structural type (recursion doesn't perturb).
- e2e (CLI): a `.mang` with a recursive `Json`/tree schema checks + hashes; the
  OpenAPI generator emits a faithful recursive type for a recursive spec and a
  `Json`-typed field for a free-form one, and `mangrove check` validates a
  conforming (nested) manifest.

## Out of scope

- Recursive `fn`/evaluation (§6.2 — stays non-recursive; that's where recursion
  genuinely breaks termination).
- Coinductive/equirecursive subtyping (recursive types simply aren't narrowable).
