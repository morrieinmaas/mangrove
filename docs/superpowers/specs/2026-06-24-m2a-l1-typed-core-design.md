# M2a — L1 Typed Core: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M2a (first of three L1 slices; M2b = units/brands, M2c = match/require/annotations).
> **Normative source:** `mangrove-spec.md` §4.1–4.4 (schema binding, type grammar, refinements, unions, custom types), §12 (structured validation errors).
> **Goal:** A self-contained `.mang` file can define types, bind one as its schema, and be **validated** — `mangrove check <file>` reports either OK or structured errors. No inference: the schema is the sole authority on type.

Builds on M1 (L0): the lexer/parser/value-model/canonical-hash are reused unchanged. M2a adds a type grammar, a validator, and structured errors.

---

## 1. Scope

**In:** the type-expression grammar; `type Name = <type>` definitions; refinement types (interval bounds, regex); union types incl. literal enums; record / map / list / named types; optional fields; binding a locally-defined type via `schema <Name>`; validating a document's value against its schema; structured validation errors (§12); the `mangrove check` command.

**Out (deferred):**
- Units & brands → **M2b** (and unit-normalization folding into canonical form).
- **Default values** (`field: T | *value`, per the §6.1/§4.7 worked examples — *not* the `= value` of §4.2; see Decision D7) → deferred to a later slice. Defaults interact with §7-step-3 materialization (D9), so they land together. In M2a a field is either required or optional, nothing more.
- `match`, `require`, metadata annotations (`@doc`/`@message`/`@deprecated`) → **M2c**. (M2a errors carry the violated constraint but not custom `@message` text yet.)
- Imports / `use` / cross-file schema resolution / lockfile → **M3 (L2)**. M2a schemas are self-contained in one file.
- Defaults materialization into canonical form (§7 step 3) and `##`/`#!` hashing (M1 D5) → still deferred; `hash` stays the L0 data hash (Decision D9).

---

## 2. Crates

- **New:** `mangrove-typed` — the `Type` AST, the type-grammar parser, and the validator. deps: `mangrove-core`, `mangrove-syntax` (reuses the lexer token stream).
- **Extend:** `mangrove-syntax/lexer.rs` — add tokens needed by the type grammar: `&`, `|`, `=`, `=~`, `?`, `>=`, `<=`, `>`, `<`. (`type`/`schema` are lexed as ordinary barewords; the parser recognizes them by position.)
- **Extend:** `mangrove-core/error.rs` — flesh out `ValidationError` to the §12 shape (path, got, expected-type, failed-constraint, position).
- **Extend:** `mangrove-cli` — add `mangrove check <file>`.
- **Extend:** `mangrove-conformance` — add an `(input → expected-errors)` vector kind under `tests/conformance/l1/`.

---

## 3. The type AST (`mangrove-typed`)

```rust
pub enum Type {
    // primitives (§3.2)
    Int, Decimal, Str, Bool, Bytes,

    // refinements (§4.3) — only the decidable predicate kinds
    IntRange   { min: Option<BigInt>, max: Option<BigInt> },     // int & >= a & <= b
    DecRange   { min: Option<BigDecimal>, max: Option<BigDecimal> },
    StrRegex(String),                                            // str & =~ "re"

    // literals (for unions / enums)
    LitStr(String), LitInt(BigInt), LitBool(bool),

    // composites
    Record { fields: Vec<FieldDef> },   // { name: T, tls?: T }  — closed, named fields
    Map(Box<Type>),                     // { [str]: V }          — dynamic keys, uniform value
    List(Box<Type>),                    // [ T ]
    Union(Vec<Type>),                   // T | U | "dev" | "prod"

    Named(String),                      // reference to a `type X = …`
}

pub struct FieldDef { pub name: String, pub optional: bool, pub ty: Type }
```

A `TypeEnv` maps names → `Type` (the file's `type` definitions). `Named` is resolved against it during validation (with cycle detection — a `type A = A` self-reference errors at load).

---

## 4. Grammar additions (per spec §4.2 + Appendix A)

```ebnf
typedef    = "type" , name , "=" , type ;
type       = union ;
union      = intersection , { "|" , intersection } ;
intersection = atom , { "&" , refinement } ;          (* & is type-only here, §5.5 *)
refinement = ( ">=" | "<=" | ">" | "<" ) , number
           | "=~" , string ;
atom       = primitive | name | record-type | list-type | map-type | literal ;
record-type= "{" , { field [ sep ] } , "}" ;
field      = name , [ "?" ] , ":" , type ;             (* ? = optional; no default in M2a *)
map-type   = "{" , "[" , "str" , "]" , ":" , type , "}" ;
list-type  = "[" , type , "]" ;
literal    = string | int | bool ;
```

A record-type is distinguished from a map-type by its first token after `{`: `[` → map, a name → record (spec §3.2). Empty `{}` is an empty record.

**Refinement–atom compatibility (Decision D10, enforced at type-load):** a refinement must match its atom's kind — interval bounds (`>= <= > <`) apply only to `int`/`decimal`, regex (`=~`) only to `str`. `str & >= 1` or `int & =~ "re"` is a **type error at load**, not a silent no-op. The refined atom collapses into the corresponding refined `Type` (`int & >=1 & <=10` → `IntRange{1,10}`; `str & =~ re` → `StrRegex(re)`).

**Statement disambiguation (parser lookahead):** at statement position the parser distinguishes a `type X = …` definition and a `schema X` binding from an ordinary field named `type`/`schema`. The discriminator is the token *after* the leading bareword: a `:` means a field binding (`type: "lib"`, `schema: "x"`); `type <name> =` is a typedef; `schema <name>` (bareword, no colon) is the schema binding. One token of lookahead suffices.

### Decision D7 — `:` in documents, `:`-type in schemas

The source spec is inconsistent (M1 D1): §3/§6.3 use `key: value` in documents; §4.2 uses `=` for value-only. Resolution: **context decides.**
- **Document body:** `key: value` (colon + value) — unchanged from L0, matches §6.3.
- **Type/record definition:** `field: Type` (required), `field?: Type` (optional); `type Name = <type>` binds a named type with `=`.

The parser knows its context (inside a `type`/record definition vs a document body), so the same `:` is unambiguous. The bare `env = "prod"` value-only form from §4.2 is **not** supported in documents (colon is the document form); this is the one place we pick a single rule over the spec's two.

**Defaults are not in M2a.** §4.2's `field: Type = default` and the worked examples' `field: T | *value` (§6.1/§4.7) are two different notations for the same feature; the `| *value` form is the one the actual examples use, and it is the one Mangrove adopts. Defaults are deferred (they interact with §7-step-3 canonical-form materialization, D9), so M2a parses neither `= default` nor `| *default`; a field is required or optional, full stop.

---

## 5. Schema binding (Decision D8 — self-contained schemas)

Until imports exist (M3), a schema is **local**. A file may contain `type` definitions and one `schema <Name>` statement; `<Name>` must resolve in the file's `TypeEnv`, and the rest of the document's bindings are validated against that (record) type.

```
type Port = int & >= 1 & <= 65535
type Server = { host: str, port: Port, tls?: bool }

schema Server

host: "localhost"
port: 8443
# tls omitted — legal, it is optional
```

`schema` with no matching type, or two `schema` statements, is a load error. A file with `type` defs but no `schema` is valid (a schema library — nothing to validate as a document) and `check` reports "no schema bound" as a no-op success.

---

## 6. Validation

`validate(value: &Value, ty: &Type, env: &TypeEnv) -> Vec<ValidationError>` walks value and type together, accumulating **all** errors (not fail-fast — better for humans and CI), each with a dotted path.

| Type | Value expected | Rule |
|------|----------------|------|
| `Int`/`Decimal`/`Str`/`Bool`/`Bytes` | matching `Value` kind | kind mismatch → error (no coercion, no inference) |
| `IntRange{min,max}` | `Value::Int` | kind, then bounds |
| `DecRange` | `Value::Decimal` | kind, then bounds |
| `StrRegex(re)` | `Value::Str` | kind, then regex match (value-level, §4.3) |
| `LitStr/LitInt/LitBool` | matching scalar equal to the literal | equality |
| `Record{fields}` | `Value::Map` | each required field present & valid; optional may be absent; **unknown key → error** (records are closed) |
| `Map(v)` | `Value::Map` | every entry value validated against `v` |
| `List(t)` | `Value::List` | every element validated against `t` |
| `Union(variants)` | any | matches **at least one** variant; else error listing the alternatives |
| `Named(n)` | — | resolve in `env` (cycle-guarded) and recurse |

Decision **D11 — no null**: an optional field (`tls?: T`) is satisfied by *presence-with-valid-value* or *absence*; there is no null state. A missing **required** field is an error; a missing **optional** field is fine.

Decision **D10 — refinements**: only interval bounds (`int`/`decimal`), regex on `str`, and literal/enum membership (unions of literals) are expressible in type position. Regex is validated against values via the `regex` crate; regex-subtype *containment* (§5.5, PSPACE) is **not** computed in M2a (it belongs to L2 subtyping, M3) — stated, not hidden.

---

## 7. Structured errors (§12) — `mangrove-core`

```rust
pub struct ValidationError {
    pub path: String,            // e.g. "container.port"  (root = "")
    pub got: String,             // a short rendering of the offending value
    pub expected: String,        // the expected type, as a single fact (never a disjunction unless it IS a union)
    pub failed: Option<String>,  // the specific constraint that failed, e.g. "<= 65535"
    pub message: Option<String>, // reserved for @message (M2c); None in M2a
    pub at: Option<Position>,    // file:line:col when known
}
pub struct Position { pub line: usize, pub col: usize }
```

Because there is no inference, `expected` is a single concrete type per error. `mangrove check` prints these in the §12 layout and exits 1 if any; exits 0 (and prints `ok`) otherwise.

---

## 8. CLI

`mangrove check <file>`:
- load → parse document (incl. `type` defs and `schema`) → resolve schema → validate.
- no schema bound → print `ok (no schema)`, exit 0.
- valid → print `ok`, exit 0.
- invalid → print each structured error (§12 layout), exit 1.
- parse/load error → message + exit 1.

`mangrove hash` is unchanged (L0 data hash; Decision D9).

---

## 9. Conformance corpus (§13, error-vector kind)

New `tests/conformance/l1/` with paired files: `name.mang` (a typed document) + `name.errors` (the expected structured errors as a small, stable text rendering — sorted by path, each `path | failed | expected`). A harness function `run_error_vector(input, errors)` validates and compares. Plus positive vectors that must validate clean. Covers: out-of-range int, regex miss, missing required field, unknown field, union non-match, type-kind mismatch, nested-path reporting, optional-field present/absent, map/list element errors.

---

## 10. Testing strategy

- Unit tests in `mangrove-typed`: type-grammar parser (each `Type` shape, refinements, unions, optional fields, defaults, nested), validator (one test per rule row in §6, incl. nested paths and multi-error accumulation), `Named` resolution + cycle detection.
- Lexer tests for the new tokens (`&`, `|`, `>=`, `=~`, `?`, …) and that they don't break L0 documents.
- Conformance error-vectors (§9).
- TDD throughout; gate `just ci` green. Verify clippy via `rtk proxy`/`just lint` (hooked clippy misreports — see memory).

---

## 11. Decisions to confirm

- **D7** — `:`+value in documents; `:`+type (`?` optional) in type/record defs, `type Name = <type>` for named types; bare `=`-value not supported in documents; **defaults (`| *value`) deferred entirely from M2a**.
- **D8** — M2a schemas are self-contained (local `type` defs + `schema <Name>`); imports deferred to M3.
- **D9** — `hash` stays the L0 data hash; defaults-materialization and unit-normalization into canonical form deferred.
- **D10** — refinements limited to interval/regex/enum; regex validated at value level, no subtype containment yet.
- **D11** — optional vs required fields; no null; unknown keys in a record are errors (records closed).
