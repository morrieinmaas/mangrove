# M2c — require, annotations & defaults: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M2c (final L1 slice; completes L1 / spec §4).
> **Normative source:** `mangrove-spec.md` §4.7 (`require`, defaults), §4.9 (annotations), §12 (errors), §7 (materialization).
> **Goal:** Complete L1 — cross-field `require` predicates, metadata annotations (`@doc`/`@message`/`@deprecated`), and field defaults (`| *value`). After M2c the entire L1 spec is implemented; `match` is the only §4 construct deferred (to M4, where `params` give it a scrutinee).

Builds on M2a (typed core) + M2b (units/brands, resolve pass). `match` (§4.8) is **out** — it switches over a `params`/reference scrutinee that does not exist until L3, so exhaustiveness checking lands in M4 (Decision D15).

---

## 1. Scope

**In:**
- **`require`** (§4.7): a total, side-effect-free predicate over a record's fields, declared in the record type, evaluated against concrete values at validation; a failure is a §12 error carrying the predicate's `@message`.
- **Annotations** (§4.9): `@doc(str)`, `@message(str)`, `@deprecated(str)` on typedefs and record fields. `@message` is surfaced in §12 errors; `@deprecated` emits an advisory; `@doc` is carried metadata.
- **Defaults** (§4.7): `field: T | *value` — an absent field materializes its default in the resolved canonical form (completing §7 step 3, begun by the M2b resolve pass).

**Out (deferred):**
- `match` + exhaustiveness → **M4** (needs a `params`/reference scrutinee; D15).
- `require` *implication* between types (subtype checking) → L2/M3; M2c only **evaluates** requires against values, never proves implication (§5.5 limit, restated).
- Regex (`=~`) and set-membership (`in`) **inside** `require` → deferred; M2c's predicate language covers comparisons, boolean ops, `len`, and field paths (the §4.7 worked examples). Stated, not hidden.

---

## 2. `require` (§4.7)

A record type may carry `require:` clauses:

```
type Listen = {
  host:  str
  port:  Port
  tls:   bool
  certs: [ str ]
  require: tls == false || len(certs) >= 1   @message("tls requires at least one cert")
  require: host != "0.0.0.0" || tls == true
}
```

**Predicate sublanguage** (total, decidable, no user functions, no recursion — §4.7):
```
pred   = or
or     = and { "||" and }
and    = cmp { "&&" cmp }
cmp    = unary [ ("=="|"!="|"<"|"<="|">"|">=") unary ]
unary  = "!" unary | operand
operand= path | literal | "len" "(" path ")" | "(" pred ")"
path   = ident { "." ident }          # references fields of the record in scope
literal= int | decimal | str | bool
```

- **AST:** `Type::Record` gains `requires: Vec<Require>` where `Require { pred: Pred, message: Option<String> }`, and `Pred` is the expression tree above.
- **Evaluation** (validator): after a record's fields validate, each `require` is evaluated against the concrete record value. A `path` resolves a field by walking the value map (a missing/!-typed operand → the require fails with a clear error, not a panic). Comparisons are kind-aware (int/decimal/str/bool); cross-kind comparison (`"a" == 1`) → the require errors. `len(path)` is the element count of a list or map, or string length. Result must be `bool`.
- A failing `require` → one §12 error at the record's path, `failed: "require"`, `message:` from `@message` if present.
- Per §4.7/§5.5: requires are **evaluated against values, never implication-checked** between types.

---

## 3. Annotations (§4.9)

```
type Port = int & >= 1 & <= 65535
  @doc("listening port")
  @message("port must be between 1 and 65535")

image: str  @deprecated("use image_ref")
```

- **Lexer:** `@` (`Tok::At`); an annotation is `@ ident ( str )` (single string arg in M2c).
- **Where they attach:** a typedef (`type X = <type> @doc(..) @message(..)`) and a record field (`field: T @deprecated(..)`).
- **Representation:** annotations are not part of the structural `Type` (so they never affect the canonical hash of *data*). They are stored beside the type: a typedef's annotations live in the `TypeEnv` keyed by type name; a field's annotations live on `FieldDef`.
- **`@message`**: when validation fails against a *named* type that carries `@message`, the error's `message` field is set to it (overriding the default). When a `require` carries `@message`, its failure uses it.
- **`@deprecated`**: `mangrove check` emits an advisory line (to stderr) for each present field whose type/field is `@deprecated`; never an error, never affects exit code.
- **`@doc`**: stored, surfaced by future tooling (hover/docs); no runtime effect in M2c.
- **Decision D17:** annotations are metadata, never in the data hash. (Per M1 D5, `##` doc-comment hashing is still deferred; `@doc` is the keyword form and is likewise out of the data hash.)

---

## 4. Defaults (§4.7)

```
type Deployment = {
  namespace: str | *"default"
  replicas:  int & >= 1 & <= 100 | *1
  expose?:   bool | *false
}
```

- **Syntax:** `field: <type> | *<value>`. The `*` (`Tok::Star`) marks the trailing default. The type parser's union loop must **not** consume a `|` that is immediately followed by `*` — that `|` belongs to the field's default, not the type's union. The field parser, after the type, consumes `| *` and parses the default value.
- **AST:** `FieldDef` gains `default: Option<Value>`.
- **Load-time check:** a default value is validated against its field's type when the schema is built; an ill-typed default (`port: Port | *0` where `Port` excludes 0) is a load error.
- **Validation:** an absent field that has a default is **valid** (the default applies); an absent field with neither default nor `?` is the existing "required field missing" error.
- **Materialization (D18, completes §7 step 3):** the `resolve` pass fills an absent defaulted field with its default value, so the canonical form (and content address) of two documents — one omitting the field, one writing the default explicitly — are **identical**. This is the §7 "defaults materialized" step, now implemented.
- Interaction with `?`: a field may be optional **or** have a default, not both meaningfully — `field?: T | *v` is redundant; M2c treats a defaulted field as satisfiable-when-absent (like optional) but *materializes* the default (unlike a bare optional, which stays absent). A bare optional absent field is **not** materialized (stays absent — there is nothing to fill).

---

## 5. Crates touched

- `mangrove-syntax`: lexer `At`, `Star`, `Dot`, `EqEq`, `Bang`, `AmpAmp`, `PipePipe` tokens; predicate parser; annotation parsing; default parsing; `Type::Record.requires`, `FieldDef.default`/`.annotations`, typedef annotations.
- `mangrove-typed`: `require` evaluator; default load-checks + materialization in `resolve`; `@message` wiring into §12 errors; `@deprecated` advisories surfaced via a returned warnings list.
- `mangrove-core`: `ValidationError.message` already exists (M2a) — now populated.
- `mangrove-cli`: `check` prints `@deprecated` advisories; defaults materialized before `hash`.
- `mangrove-conformance`: L1 vectors for require pass/fail, `@message` surfacing, default materialization (hash-equivalence of omitted vs explicit), deprecation advisory.

---

## 6. Decisions to confirm

- **D15** — `match` deferred to M4 (needs an L3 scrutinee).
- **D16** — `require` predicate language = comparisons + `&&`/`||`/`!` + `len` + field paths + parens; regex/`in` inside require deferred; evaluated against values, never implication-checked.
- **D17** — annotations (`@doc`/`@message`/`@deprecated`) are metadata, never in the data hash; `@message` populates §12 errors, `@deprecated` is an advisory, `@doc` is carried.
- **D18** — defaults `| *value` materialize into the resolved canonical form (completes §7 step 3); a bare optional (`?`) absent field is not materialized; default values are type-checked at load.
