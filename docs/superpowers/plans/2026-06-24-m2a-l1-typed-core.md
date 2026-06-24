# M2a — L1 Typed Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline) or subagent-driven-development. Steps use `- [ ]` checkboxes. Design: `../specs/2026-06-24-m2a-l1-typed-core-design.md`.

**Goal:** `mangrove check <file>` validates a self-contained typed `.mang` document (local `type` defs + `schema <Name>` + body) against its schema, emitting structured §12 errors or `ok`.

**Architecture:** Reuse M1 (lexer/value-parser/value-model/hash) unchanged for the body. Add type-grammar tokens to the lexer, a `Type` AST + type/document parser to **`mangrove-syntax`** (all parsing lives in the parser crate — this avoids a `syntax ↔ typed` cycle, since the document body is L0 values), and a new **`mangrove-typed`** crate holding only semantics (TypeEnv resolution + validator). `ValidationError` in `mangrove-core` is fleshed to the §12 shape.

**Tech Stack:** Rust 2024; `regex` (new dep, for `=~` value validation); reuses `num-bigint`/`bigdecimal`.

## Global Constraints

- Edition 2024, Apache-2.0, workspace inheritance, `unsafe_code = "forbid"`, clippy `-D warnings`.
- Gate after each task: `just ci` green. Verify clippy via `rtk proxy cargo clippy` / `just lint` (hooked clippy misreports — see memory).
- Commit directly to `main` (trunk-based).
- Decisions D7–D11 (see spec): `:`+value in docs / `:`+type in defs; self-contained schemas; `hash` stays L0 data hash; refinements = interval/regex/enum, regex value-level only; optional-vs-required, no null, closed records. **No defaults in M2a.**
- No inference anywhere: kind mismatch is an error, never a coercion.

## Workspace dependency addition

Add to root `Cargo.toml` `[workspace.dependencies]`: `regex = "1"`.

---

### Task 1: Lexer — type-grammar tokens

**Files:** Modify `crates/mangrove-syntax/src/lexer.rs`.

**Interfaces — Produces:** new `Tok` variants `Amp` (`&`), `Pipe` (`|`), `Eq` (`=`), `Match` (`=~`), `Question` (`?`), `Ge` (`>=`), `Le` (`<=`), `Gt` (`>`), `Lt` (`<`). L0 token stream for existing documents is unchanged.

- [ ] **Step 1: Write failing tests** (in `lexer.rs` tests):
```rust
    #[test]
    fn type_grammar_tokens() {
        use Tok::*;
        assert_eq!(
            toks("a & b | c =~ d ? >= <= > <"),
            vec![
                Bareword("a".into()), Amp, Bareword("b".into()), Pipe, Bareword("c".into()),
                Match, Bareword("d".into()), Question, Ge, Le, Gt, Lt, Eof
            ]
        );
    }
    #[test]
    fn ge_le_match_are_two_char_greedy() {
        // ">=" not ">" "=", "=~" not "=" "~", "<=" not "<" "="
        assert_eq!(toks(">="), vec![Tok::Ge, Tok::Eof]);
        assert_eq!(toks("=~"), vec![Tok::Match, Tok::Eof]);
        assert_eq!(toks("="), vec![Tok::Eq, Tok::Eof]);
    }
```
- [ ] **Step 2: Run** `cargo test -p mangrove-syntax type_grammar` → FAIL (unknown variants).
- [ ] **Step 3: Implement** — add the variants to `enum Tok`. In the lexer `run()` match, add arms (order matters — check two-char before one-char):
  - `'&'` → `Amp`; `'|'` → `Pipe`; `'?'` → `Question`.
  - `'='` → if `peek_at(1) == Some('~')` bump both → `Match`, else bump → `Eq`.
  - `'>'` → if `peek_at(1) == Some('=')` bump both → `Ge`, else `Gt`.
  - `'<'` → if `peek_at(1) == Some('=')` bump both → `Le`, else `Lt`.
  Place these arms before the `is_ident_start`/number arms (none start with these chars, so order vs. those is irrelevant, but keep punctuation together).
- [ ] **Step 4: Run** `cargo test -p mangrove-syntax` → PASS (new + all existing). `just fmt`.
- [ ] **Step 5: Commit** `feat(syntax): lex type-grammar tokens (& | = =~ ? >= <= > <)`.

---

### Task 2: Type AST + type-expression parser

**Files:** Create `crates/mangrove-syntax/src/ty.rs`; modify `src/parser.rs` (add type-parsing methods), `src/lib.rs`.

**Interfaces — Produces:** `mangrove_syntax::ty::{Type, FieldDef}` (per design §3) and `mangrove_syntax::parse_type(&str) -> Result<Type, ParseError>` (test entrypoint that lexes then parses one type expression).

```rust
// ty.rs
pub enum Type {
    Int, Decimal, Str, Bool, Bytes,
    IntRange { min: Option<BigInt>, max: Option<BigInt> },
    DecRange { min: Option<BigDecimal>, max: Option<BigDecimal> },
    StrRegex(String),
    LitStr(String), LitInt(BigInt), LitBool(bool),
    Record { fields: Vec<FieldDef> },
    Map(Box<Type>),
    List(Box<Type>),
    Union(Vec<Type>),
    Named(String),
}
pub struct FieldDef { pub name: String, pub optional: bool, pub ty: Type }
```

- [ ] **Step 1: Write failing tests** (`ty.rs` tests, calling `crate::parser::parse_type`):
```rust
    #[test] fn primitive() { assert_eq!(pt("int"), Type::Int); }
    #[test] fn int_range() {
        assert_eq!(pt("int & >= 1 & <= 10"),
            Type::IntRange { min: Some(1.into()), max: Some(10.into()) });
    }
    #[test] fn str_regex() {
        assert_eq!(pt("str & =~ \"^a+$\""), Type::StrRegex("^a+$".into()));
    }
    #[test] fn union_of_literals() {
        assert_eq!(pt("\"dev\" | \"prod\""),
            Type::Union(vec![Type::LitStr("dev".into()), Type::LitStr("prod".into())]));
    }
    #[test] fn record_with_optional() {
        let t = pt("{ host: str, port: int, tls?: bool }");
        let Type::Record { fields } = t else { panic!() };
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().find(|f| f.name == "tls").unwrap().optional);
        assert!(!fields.iter().find(|f| f.name == "host").unwrap().optional);
    }
    #[test] fn map_and_list() {
        assert_eq!(pt("{ [str]: int }"), Type::Map(Box::new(Type::Int)));
        assert_eq!(pt("[ str ]"), Type::List(Box::new(Type::Str)));
    }
    #[test] fn named() { assert_eq!(pt("Port"), Type::Named("Port".into())); }
    #[test] fn refinement_atom_mismatch_errors() {
        // F2 / D10: bounds only on int/decimal, regex only on str
        assert!(crate::parse_type("str & >= 1").is_err());
        assert!(crate::parse_type("int & =~ \"re\"").is_err());
    }
    // helper: fn pt(s: &str) -> Type { crate::parse_type(s).unwrap() }
```
- [ ] **Step 2: Run** `cargo test -p mangrove-syntax ty::` → FAIL.
- [ ] **Step 3: Implement** the type parser in `parser.rs` (precedence: `union` = `intersection { | intersection }`; `intersection` = `atom { & refinement }`). `atom` dispatches on the next token: primitives via bareword keywords (`int`/`decimal`/`str`/`bool`/`bytes`), other bareword → `Named`, `{` → record-or-map (peek: `[` after `{` → map, else record; record fields parse `name [?] : type`), `[` → list, string/int/bool literal → `Lit*`. Refinements fold into the atom: a `Ge`/`Le`/`Gt`/`Lt` + number refines `Int`→`IntRange`/`Decimal`→`DecRange` (error on any other atom — F2); `=~` + string refines `Str`→`StrRegex` (error otherwise — F2). `parse_type` (public in `lib.rs`) lexes then parses one type to EOF. Add `pub mod ty;` and `pub use ty::{Type, FieldDef};` and `pub use parser::parse_type;` to `lib.rs`.
- [ ] **Step 4: Run** `cargo test -p mangrove-syntax` → PASS. `just fmt`.
- [ ] **Step 5: Commit** `feat(syntax): L1 type-expression grammar`.

---

### Task 3: Document parser (typedefs + schema + body)

**Files:** Modify `crates/mangrove-syntax/src/parser.rs`, `src/lib.rs`.

**Interfaces — Produces:** `mangrove_syntax::Document { typedefs: Vec<(String, Type)>, schema: Option<String>, body: Value }` and `mangrove_syntax::parse_document(&str) -> Result<Document, ParseError>`. The existing `parse(&str) -> Result<Value, ParseError>` is kept (now `= parse_document(..).map(|d| d.body)`), so `mangrove hash` and all L0 vectors are unaffected.

- [ ] **Step 1: Write failing tests** (`parser.rs` tests):
```rust
    #[test]
    fn parses_typedefs_schema_and_body() {
        let d = parse_document(
            "type Port = int & >= 1 & <= 65535\nschema Server\nhost: \"x\"\nport: 8443"
        ).unwrap();
        assert_eq!(d.typedefs.len(), 1);
        assert_eq!(d.typedefs[0].0, "Port");
        assert_eq!(d.schema.as_deref(), Some("Server"));
        let Value::Map(m) = &d.body else { panic!() };
        assert!(m.contains_key("host") && m.contains_key("port"));
    }
    #[test]
    fn field_named_type_or_schema_is_a_binding_not_a_statement() {
        // F3: `type:`/`schema:` (colon) → ordinary field, not a typedef/schema-binding
        let d = parse_document("type: \"lib\"\nschema: \"x\"").unwrap();
        assert!(d.typedefs.is_empty() && d.schema.is_none());
        let Value::Map(m) = &d.body else { panic!() };
        assert_eq!(m.get("type"), Some(&Value::Str("lib".into())));
        assert_eq!(m.get("schema"), Some(&Value::Str("x".into())));
    }
    #[test]
    fn pure_l0_document_has_no_typedefs_or_schema() {
        let d = parse_document("a: 1\nb: 2").unwrap();
        assert!(d.typedefs.is_empty() && d.schema.is_none());
    }
    #[test]
    fn two_schema_statements_is_error() {
        assert!(parse_document("schema A\nschema B").is_err());
    }
```
- [ ] **Step 2: Run** `cargo test -p mangrove-syntax parses_typedefs` → FAIL.
- [ ] **Step 3: Implement** — at top-level statement position, look at the leading token: if `Bareword("type")` AND the next token is a `Bareword` (not `Colon`) → parse a typedef (`type Name = <type>`, using Task 2's type parser); if `Bareword("schema")` AND next is `Bareword` → record the schema name (error if already set); otherwise parse an ordinary `key: value` binding into `body`. Build and return `Document`. Refactor `parse_document` so `parse` delegates to it. Body bindings still go through the existing dedup/separator logic.
- [ ] **Step 4: Run** `cargo test -p mangrove-syntax` (incl. all L0 + Task 2) → PASS. `just fmt`.
- [ ] **Step 5: Commit** `feat(syntax): parse typedefs, schema binding, and body into Document`.

---

### Task 4: `mangrove-typed` crate — TypeEnv & resolution

**Files:** Create `crates/mangrove-typed/{Cargo.toml,src/lib.rs,src/env.rs}`.

**Interfaces — Produces:** `mangrove_typed::TypeEnv` with `TypeEnv::build(typedefs: &[(String, Type)]) -> Result<TypeEnv, String>` (errors on duplicate type name or a `Named` cycle) and `resolve(&self, name: &str) -> Option<&Type>`.

- [ ] **Step 1: Write failing tests** (`env.rs`):
```rust
    #[test]
    fn resolves_named_types() {
        let env = TypeEnv::build(&[("Port".into(), Type::Int)]).unwrap();
        assert_eq!(env.resolve("Port"), Some(&Type::Int));
    }
    #[test]
    fn duplicate_type_name_errors() {
        assert!(TypeEnv::build(&[("A".into(), Type::Int), ("A".into(), Type::Str)]).is_err());
    }
    #[test]
    fn direct_cycle_errors() {
        // type A = A  (totality: no recursive types)
        assert!(TypeEnv::build(&[("A".into(), Type::Named("A".into()))]).is_err());
    }
```
- [ ] **Step 2: Manifest** `crates/mangrove-typed/Cargo.toml`:
```toml
[package]
name = "mangrove-typed"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
mangrove-core = { path = "../mangrove-core" }
mangrove-syntax = { path = "../mangrove-syntax" }
regex = { workspace = true }
num-bigint = { workspace = true }
bigdecimal = { workspace = true }

[lints]
workspace = true
```
- [ ] **Step 3: Run** `cargo test -p mangrove-typed` → FAIL (no crate yet / no `TypeEnv`).
- [ ] **Step 4: Implement** `env.rs`: `TypeEnv(HashMap<String, Type>)`; `build` inserts each typedef (error on duplicate key), then does a DFS over every `Named` reference following `Record`/`Map`/`List`/`Union` children, tracking a visited-stack to detect cycles (error). `resolve` is a map lookup. Add `pub mod env; pub use env::TypeEnv;` to `lib.rs`.
- [ ] **Step 5: Run** `cargo test -p mangrove-typed` → PASS.
- [ ] **Step 6: Commit** `feat(typed): TypeEnv with named-type resolution and cycle detection`.

---

### Task 5: Validator + §12 ValidationError

**Files:** Modify `crates/mangrove-core/src/error.rs` (flesh `ValidationError`); create `crates/mangrove-typed/src/validate.rs`; modify `src/lib.rs`.

**Interfaces — Produces:**
- `mangrove_core::error::{ValidationError, Position}` per design §7 (`path`, `got`, `expected`, `failed: Option<String>`, `message: Option<String>`, `at: Option<Position>`). Keep `ValidationError::new(path, message)`-style construction available via a builder or direct struct literal; update the existing M0 test accordingly.
- `mangrove_typed::validate(value: &Value, ty: &Type, env: &TypeEnv) -> Vec<ValidationError>` — empty Vec ⇒ valid; accumulates all errors with dotted paths.

- [ ] **Step 1: Write failing tests** (`validate.rs`), one per design-§6 rule:
```rust
    use super::validate;
    use mangrove_syntax::{parse_type, Type};
    use mangrove_typed::TypeEnv;
    use mangrove_core::Value;
    use num_bigint::BigInt;

    fn ty(s: &str) -> Type { parse_type(s).unwrap() }
    fn env() -> TypeEnv { TypeEnv::build(&[]).unwrap() }
    fn errs(v: Value, t: &str) -> Vec<mangrove_core::error::ValidationError> {
        validate(&v, &ty(t), &env())
    }

    #[test] fn int_in_range_ok() { assert!(errs(Value::Int(5.into()), "int & >= 1 & <= 10").is_empty()); }
    #[test] fn int_out_of_range_errs() {
        let e = errs(Value::Int(70000.into()), "int & >= 1 & <= 65535");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].failed.as_deref(), Some("<= 65535"));
    }
    #[test] fn kind_mismatch_errs_no_coercion() {
        assert_eq!(errs(Value::Str("5".into()), "int").len(), 1);
    }
    #[test] fn regex_miss_errs() {
        assert_eq!(errs(Value::Str("A".into()), "str & =~ \"^[a-z]+$\"").len(), 1);
    }
    #[test] fn union_membership() {
        assert!(errs(Value::Str("dev".into()), "\"dev\" | \"prod\"").is_empty());
        assert_eq!(errs(Value::Str("x".into()), "\"dev\" | \"prod\"").len(), 1);
    }
    #[test] fn record_missing_required_and_unknown_key() {
        // missing required `port`, plus unknown `extra`
        let mut m = std::collections::BTreeMap::new();
        m.insert("host".to_string(), Value::Str("h".into()));
        m.insert("extra".to_string(), Value::Bool(true));
        let e = errs(Value::Map(m), "{ host: str, port: int }");
        assert_eq!(e.len(), 2); // missing port + unknown extra
    }
    #[test] fn optional_absent_ok_present_checked() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("host".to_string(), Value::Str("h".into()));
        assert!(validate(&Value::Map(m), &ty("{ host: str, tls?: bool }"), &env()).is_empty());
    }
    #[test] fn nested_path_reported() {
        let mut inner = std::collections::BTreeMap::new();
        inner.insert("port".to_string(), Value::Int(70000.into()));
        let mut m = std::collections::BTreeMap::new();
        m.insert("listen".to_string(), Value::Map(inner));
        let e = validate(&Value::Map(m), &ty("{ listen: { port: int & <= 65535 } }"), &env());
        assert_eq!(e[0].path, "listen.port");
    }
    #[test] fn list_and_map_element_errors() {
        let l = Value::List(vec![Value::Int(1.into()), Value::Str("x".into())]);
        assert_eq!(validate(&l, &ty("[ int ]"), &env()).len(), 1);
    }
```
- [ ] **Step 2: Run** `cargo test -p mangrove-typed validate` → FAIL.
- [ ] **Step 3: Implement** `ValidationError`/`Position` in core (struct with the §7 fields; add a `Default`-ish constructor or just use struct literals in the validator; fix the M0 `carries_path_and_message` test to the new shape). Implement `validate` in typed: a recursive walk `(value, type, path) -> push errors`, matching the design §6 table. `Named` → `env.resolve` then recurse (resolution already cycle-safe from Task 4). Build `path` as dotted (`""` root, `parent.child`, list uses `parent[idx]`). Regex via `regex::Regex::new(re)` (a bad regex in a type is a load/validation error). Accumulate, never fail-fast. Add `pub mod validate; pub use validate::validate;`.
- [ ] **Step 4: Run** `cargo test -p mangrove-typed && cargo test -p mangrove-core` → PASS.
- [ ] **Step 5: Commit** `feat(typed): structured validator (§12 errors) for L1 types`.

---

### Task 6: CLI `mangrove check`

**Files:** Modify `crates/mangrove-cli/{Cargo.toml,src/main.rs,tests/cli.rs}`.

**Interfaces — Produces:** `mangrove check <file>` → resolve schema, validate body, print `ok` / `ok (no schema)` / structured errors; exit 0 (valid) or 1 (invalid or parse error).

- [ ] **Step 1: Write failing tests** (`tests/cli.rs`):
```rust
#[test]
fn check_valid_document_exits_0() {
    let p = std::env::temp_dir().join("m2a_ok.mang");
    std::fs::write(&p, "type Port = int & >= 1 & <= 65535\nschema Server\nhost: \"h\"\nport: 8443\n").unwrap();
    // Server must be defined; use an inline record type named Server:
    std::fs::write(&p, concat!(
        "type Server = { host: str, port: int & >= 1 & <= 65535 }\n",
        "schema Server\n",
        "host: \"h\"\nport: 8443\n"
    )).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove")).arg("check").arg(&p).output().unwrap();
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
}
#[test]
fn check_invalid_document_exits_1() {
    let p = std::env::temp_dir().join("m2a_bad.mang");
    std::fs::write(&p, concat!(
        "type Server = { host: str, port: int & >= 1 & <= 65535 }\n",
        "schema Server\n",
        "host: \"h\"\nport: 70000\n"
    )).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove")).arg("check").arg(&p).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("port"));
}
#[test]
fn check_no_schema_is_ok() {
    let p = std::env::temp_dir().join("m2a_noschema.mang");
    std::fs::write(&p, "a: 1\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove")).arg("check").arg(&p).output().unwrap();
    assert!(out.status.success());
}
```
- [ ] **Step 2: Add dep** `mangrove-typed = { path = "../mangrove-typed" }` to cli `Cargo.toml`.
- [ ] **Step 3: Run** `cargo test -p mangrove-cli check_` → FAIL.
- [ ] **Step 4: Implement** a `check` arm in `main.rs`: read file → `mangrove_syntax::parse_document` → `TypeEnv::build(&doc.typedefs)` (load error → stderr+exit 1) → if `doc.schema` is `None` print `ok (no schema)` exit 0 → else `env.resolve(name)` (unknown → error exit 1) → `validate(&doc.body, schema_ty, &env)` → if empty print `ok` exit 0, else print each error (§12 layout: `path`, `got`, `expected`, `failed`, `at`) and exit 1.
- [ ] **Step 5: Run** `cargo test -p mangrove-cli` → PASS.
- [ ] **Step 6: Commit** `feat(cli): add check subcommand`.

---

### Task 7: Conformance — L1 error vectors

**Files:** Modify `crates/mangrove-conformance/{Cargo.toml,src/lib.rs,tests/corpus.rs}`; add `tests/conformance/l1/` vectors.

**Interfaces — Produces:** `mangrove_conformance::run_check_vector(input: &Path, expected: &Path)` — parse+resolve+validate `input`, render errors to the stable text form, compare to `expected.trim()`. A `.ok` expected file means "must validate clean".

- [ ] **Step 1: Write failing test** (`tests/corpus.rs`):
```rust
const L1_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/l1");

#[test]
fn all_l1_vectors_match_expected() {
    for (input, expected) in vector_pairs(Path::new(L1_CORPUS)) {
        mangrove_conformance::run_check_vector(&input, &expected);
    }
}
```
(Reuse `vector_pairs`; the `.expected` extension already pairs with `.mang`.)
- [ ] **Step 2: Add deps** `mangrove-typed` to conformance `Cargo.toml` (syntax/canonical already present from M1).
- [ ] **Step 3: Run** `cargo test -p mangrove-conformance all_l1` → FAIL (no corpus dir / no `run_check_vector`).
- [ ] **Step 4: Implement** `run_check_vector` in `src/lib.rs`: parse_document → build TypeEnv → resolve schema → validate; render the resulting errors deterministically (sorted by `path`, one line each: `path | failed | expected`), or the literal `ok` when none; assert equals `expected.trim()`.
- [ ] **Step 5: Add vectors** under `tests/conformance/l1/` (each `name.mang` + `name.expected`): `ok_basic` (validates → `ok`), `out_of_range`, `unknown_field`, `missing_required`, `union_miss`, `kind_mismatch`, `nested_path`, `optional_absent` (→ `ok`), `list_elem`. Author the `.expected` by running `mangrove check` mentally/by-eye against the rendering and pinning it; eyeball each input.
- [ ] **Step 6: Run** `cargo test -p mangrove-conformance` → PASS.
- [ ] **Step 7: Full gate** `just ci` → green. Push to `main`, verify CI.
- [ ] **Step 8: Commit** `feat(conformance): L1 validation vectors`.

---

## Self-Review

**Spec coverage (vs M2a design):** type AST → T2; type grammar §4 + refinement-atom F2 → T2; statement disambiguation F3 → T3; schema binding D8 → T3/T6; TypeEnv + Named + cycles → T4; validation rules §6 + §12 errors → T5; `check` CLI §8 → T6; error-vector corpus §9 → T7. Decisions D7 (no defaults), D9 (hash unchanged), D10 (refinements), D11 (optional/closed/no-null) all covered. ✓

**Crate-split note:** Type AST + parser live in `mangrove-syntax` (not `mangrove-typed` as the roadmap sketched) to avoid a `syntax↔typed` cycle, since the L1 body is L0 values parsed by syntax. `mangrove-typed` is pure semantics. Deliberate, documented here.

**Placeholder scan:** `.expected` vector contents (T7) are generated/eyeballed then pinned — intentional, same pattern as M1. No TODOs.

**Type consistency:** `Type`/`FieldDef` (T2) consumed by `TypeEnv` (T4) and `validate` (T5); `Document{typedefs,schema,body}` (T3) consumed by CLI (T6) and conformance (T7); `ValidationError` §12 shape (T5) used by CLI + conformance render. `parse_type`/`parse_document` entrypoints consistent across tasks.
