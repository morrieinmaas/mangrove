# M4d.2 — `emit` / module calls (design)

**Goal:** complete L3 by letting a document **call a parameterized module** (§6.1): `emit: webapp(name: "x", env: "prod")` loads the module bound to `webapp`, supplies its params from the named args, evaluates its body, and yields the resulting value. This is the cross-file counterpart to M4d.1's schema-local `fn`.

## The shape

```
use infra/charts/webapp @b3:… as webapp

emit: webapp(
  name:  "api-gateway"
  image: "registry.example.eu/api:1.21.0"
  env:   "prod"
)
```

`emit` is **not** a keyword — it is an ordinary field whose value is a module call. The new construct is the call expression `alias(named-args)`.

## Architecture — orchestrate in compose, not eval

`eval` lives in `mangrove-typed`; module loading lives in `mangrove-compose` (which depends on `mangrove-typed`). eval therefore cannot load modules. A module call needs the callee's *whole* `Composed` (its `params`, `fns`, `typedefs`/`unitdefs`, and `body`) — which `compose` already has, because the callee arrived via a `use`. So:

- **D40 — module calls are resolved in the compose layer.** `compose_rec` already builds a `Composed` for every `use`d alias. Today it keeps only `alias → body` (for spreads). It will additionally keep `alias → Composed`. After folding the body, a resolution pass walks the value tree; for each `ModuleCall { alias, args }` it:
  1. looks up the alias's `Composed` (error if the alias was never `use`d);
  2. evaluates each arg value **in the caller's scope** (so `webapp(env: stage)` can pass a caller param) — i.e. the caller's own eval must run first or the args must be pre-reduced;
  3. binds the callee's params from those args (D34: supplied wins, else default, else required-unbound error; an arg naming no param is an error);
  4. evaluates the callee's body via `mangrove_typed::eval` against the callee's *own* `TypeEnv` (built from the callee's typedefs/units) and `fns`, with the bound params;
  5. validates the result against the callee's schema if it declares one;
  6. substitutes the resulting value in place.

  Nested calls (a module that calls another) recurse through the same pass; depth-bounded like every recursive pass.

- **D41 — eval gains a `supplied` param map.** `eval(body, params, supplied, fns, types)` where `supplied: &BTreeMap<String,Value>` overrides defaults. The root document calls with an empty `supplied` (its params use defaults, M4a). A module call passes the bound args. This is the single change to the eval signature; the M4a binding rule (D34) generalizes: `supplied[name]` → else `default` → else required-unbound error; an unknown supplied name → error.

### Ordering of the two eval passes

The caller's body is evaluated (refs/match/interp/fn) to reduce its own expressions, and module calls are resolved using args taken from that same caller scope. The clean model: run the caller's eval first to reduce args within `ModuleCall` nodes to values, then resolve each `ModuleCall` by evaluating the callee. Because a `ModuleCall`'s args are arbitrary caller expressions, the resolution is interleaved: resolve a `ModuleCall` by (a) eval its args in the caller ctx, (b) eval the callee body in the callee ctx. This is exactly `reduce_call` (M4d.1) generalized across the file boundary — so the cleanest implementation makes eval itself able to resolve module calls, given a `modules: &BTreeMap<String, Module>` map (alias → callee params/body/fns/types) supplied by compose. That keeps arg-scoping correct for free and unifies fn and module calls under one `reduce`.

**Decision: thread `modules` into eval (option B).** Compose builds, for each `use`d alias, a `Module { params, fns, types, body }` (owning the callee's `TypeEnv`), collects them into `alias → Module`, and passes the map to eval alongside the root params/fns. eval's `reduce` handles `ModuleCall { alias, args }` like `reduce_call`, but switches `Ctx.types`/`Ctx.fns` to the callee's for the body. This avoids a second resolution pass and keeps caller-arg scoping identical to `fn` calls. Compose still does the *plumbing* (it owns the `Composed`s and builds the `Module` map); eval does the *reduction*.

## Parsing

- `Value::ModuleCall { alias: String, args: Vec<(String, Value)> }` (named args, ordered for error messages; bound by name).
- `parse_call` (M4d.1) currently parses positional args into `Value::Call`. Extend: after `(`, if the first token is `Bareword` immediately followed by `Colon`, parse **named** args → `ModuleCall`; otherwise positional → `Call` (fn). A call mixing named and positional is an error.
- Disambiguation of `Call` vs `ModuleCall` is purely syntactic (named vs positional args); resolution (fn vs module) is by name lookup at eval (a `fn` name vs a `use` alias).

## Canonical form

Unchanged guarantee (D12/D35): a module call reduces to an ordinary value; the hash is of the evaluated value. `emit: webapp(env: "prod")` hashes identically to the hand-written value the module produces for `env = "prod"`. `Value::ModuleCall` is a transient marker guarded in the CBOR encoder like the others.

## Testing (hermetic)

- A local module file with `params` + body; a root that `use`s it and calls it; assert the composed/evaluated value and that it hashes like the hand-written result.
- Supplied arg overrides default; unsupplied required arg → error; unknown arg name → error; arg type violation → error.
- Nested module calls; a module-call depth guard (cycle of modules calling each other) → bounded error, no overflow.
- Caller param passed as an arg (`webapp(env: stage)` where `stage` is a root param) resolves in the caller scope.
- An L0–L2 / M4a–c document is unaffected.

## Out of scope

Per-type pins (§5.6) still pending cross-file *type* imports (separate milestone). Recursion across modules is bounded-and-errored, not supported.
