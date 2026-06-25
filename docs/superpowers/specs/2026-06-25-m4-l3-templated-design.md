# M4 — L3 Templated (design)

**Goal:** add the L3 layer (§6): a document can be a *function of params*, with `match`, string interpolation, schema-defined `fn` constructors, and module calls (`emit`). Templating operates on **values, not text** — the output is an ordinary L2 value that validates and hashes exactly as a hand-written one.

## Architecture — a new eval stage

The pipeline gains one stage:

```
parse → compose (merge/spread/unset, L2) → EVAL (L3: refs, match, interp, fn, calls) → validate + resolve + canonical-hash
```

Eval reduces L3 *expressions* to L2 *values* against an **environment** (params + already-evaluated bindings). After eval there are no expressions left — so validate/resolve/hash (D12) are unchanged and operate on the evaluated value. The content address is the evaluated, schema-resolved value (D12 extends naturally: an expression has no hash; its reduced value does).

### Value vs Expr

Today the body is a `Value`. L3 needs *expressions* in value positions. We add an `Expr` layer the parser produces in value positions, and an `eval(expr, env) -> Value`. A plain literal is `Expr::Lit(Value)`; eval of a literal is the identity, so L0–L2 documents are unaffected (they contain only `Expr::Lit`). Expr variants: `Lit(Value)`, `Ref(name)`, `Match{scrutinee, arms}`, `Interp(parts)`, `Call{fn, args}` (M4d), `ModuleCall{...}` (M4d).

## Decisions

- **D34 — params: default = optional, no default = required (resolves the supply fork).** `params { env: "dev"|"prod" = "dev", version: str }` — `env` is optional (default `"dev"`), `version` is required. At eval: a required param with no supplied value is a hard error (`param \`version\` unbound`); an optional param falls back to its default. This single rule gives both "call-only pure module" (write no defaults → `check`/`hash` errors until called) and "standalone-runnable module" (give defaults). Reuses the §4.4 default mechanism; for hashing, unsupplied optional params materialize their defaults (D18). A required-unbound param simply has no canonical form (it is a function, not a value) — error, by design.
- **D35 — eval after compose, before validate.** Expressions are reduced once, post-compose; the result feeds the existing validate/resolve/hash path. No expression survives into the canonical form.
- **D36 — references resolve params then sibling bindings.** A bare `name` in a value position resolves to a param (if declared) else a top-level binding in the evaluated body. Lexical, no inference. Cycles among bindings → error (depth-guarded like every other recursive pass). Forward references allowed (eval bindings by dependency, or two-pass) — kept simple in M4a: resolve against params + already-bound earlier siblings, error on unknown.
- **D37 — match is total.** `match scrutinee { pat: val, … }` with literal patterns; an arm `_` is the catch-all. Required to be exhaustive: if the scrutinee's type is a known finite union, arms must cover it; otherwise a `_` arm is required. A non-exhaustive match is a compile error, never a runtime fallthrough.
- **D38 — fn: schema-defined, total, non-recursive (§6.2).** `fn port(n: int): Port = { number: n, name: "http" }` lives in a schema; callable in documents (`port: port(8443)`). The call reduces to the body with params bound; the result must satisfy the declared return type. No recursion (guarded). Documents cannot *define* fns.
- **D39 — interpolation is value-level (§6.3).** `"$name"` / `"${name}"` inside `"…"`/`"""…"""`; disabled in raw (`r"…"`). `\$` is a literal `$`. An interpolation hole reduces to the referenced value rendered into the string; it can only produce the field's typed value, never document structure. Non-string interpolation (e.g. `${port}` where port is int) renders the int's canonical text.

## Slices (each: spec note → TDD → adversarial review → CI green)

- **M4a — eval skeleton + references + `params`.** `Expr` layer, `eval(expr, env)`, the param env (D34), bare-name refs (D36). CLI `check`/`hash` gain the eval stage. Proves: a doc with params+refs evaluates; required-unbound errors; defaults materialize; an L0–L2 doc is byte-identical through the new stage.
- **M4b — interpolation (D39).** Lexer/parser for `$name`/`${name}`/`\$`, raw opt-out; `Expr::Interp`; eval renders parts.
- **M4c — `match` (D37).** Parser for `match … { … }`; `Expr::Match`; exhaustiveness check; eval selects the arm.
- **M4d — `fn` (D38) + `emit`/module calls.** Schema `fn` defs; `Expr::Call`; module call (`emit: webapp(...)`) loads + binds + evaluates another module. Largest slice; may sub-split.

## Testing posture

Each slice is hermetic and TDD. Eval has its own unit tests (expr × env → value); end-to-end CLI tests prove `hash` of a templated doc equals `hash` of the hand-written evaluated form (D12). Every recursive pass (ref resolution, match, fn, module calls) gets a depth guard with a test, per the standing pattern.

## Out of scope / deferred

- Per-type pins (§5.6) still pending cross-file type imports.
- Recursion in fns (§6.2 says non-recursive — enforced, not supported).
- Overloading (§6.4 — explicitly not supported).
