# M2b — Units, Brands & Resolved Canonical Form Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline). Steps use `- [ ]`. Design: `../specs/2026-06-24-m2b-units-brands-design.md`.

**Goal:** Unit types + literals (`512Mi`, `0.5btc`), `brand` types, and the content address reframed as the schema-**resolved** canonical form (units → base int; L0 unchanged).

**Architecture:** Lexer emits unit-literal tokens; `Value::Unit{mantissa,suffix}` carries them unresolved; `mangrove-typed` resolves them against a field's unit type (suffix membership, exact-integer base, range) in a `resolve` pass; the content address is `hash(resolve(doc, schema))`, which for schemaless docs equals the raw data (M1 hashes preserved).

**Tech Stack:** Rust 2024; reuses `num-bigint`/`bigdecimal`/`blake3`/`regex`.

## Global Constraints

- Edition 2024, Apache-2.0, workspace inheritance, `unsafe_code = "forbid"`, clippy `-D warnings`.
- Gate per task: `just ci` green; clippy via `rtk proxy cargo clippy` / `just lint`.
- Commit directly to `main`.
- Decisions: **D12** content address = resolved canonical form (L0 unchanged); **D13** unit member arithmetic = `coefficient × earlier-member`; **D14** unit literal needs a unit-typed context (schemaless → error). Brand identity is schema-level, never in the data hash.
- **No M1 hash may change** — the L0 conformance corpus is the regression guard.

---

### Task 1: Lexer — unit-literal token (reverses M1 D3)

**Files:** Modify `crates/mangrove-syntax/src/lexer.rs`.

**Interfaces — Produces:** `Tok::UnitLit(BigDecimal, String)` = `(mantissa, suffix)`. A number immediately followed by an identifier now lexes as `UnitLit` instead of erroring.

- [ ] **Step 1: Update the existing test + add new** — change `unit_suffix_is_error` (which asserted `lex("a: 512Mi").is_err()`) to expect a token, and add:
```rust
    #[test]
    fn unit_literal_lexes() {
        use bigdecimal::BigDecimal;
        use std::str::FromStr;
        assert_eq!(
            toks("a: 512Mi").get(2),
            Some(&Tok::UnitLit(BigDecimal::from(512), "Mi".into()))
        );
        assert_eq!(
            toks("x: 0.5btc").get(2),
            Some(&Tok::UnitLit(BigDecimal::from_str("0.5").unwrap(), "btc".into()))
        );
        // underscores in the numeric part still strip
        assert_eq!(
            toks("x: 100_000_000sat").get(2),
            Some(&Tok::UnitLit(BigDecimal::from(100_000_000), "sat".into()))
        );
    }
```
Delete/replace the old `unit_suffix_is_error` test.
- [ ] **Step 2: Run** `cargo test -p mangrove-syntax unit_literal` → FAIL.
- [ ] **Step 3: Implement** — add `UnitLit(BigDecimal, String)` to `enum Tok`. In `lex_number`, where it currently errors on a trailing ident char, instead read the identifier suffix (`[A-Za-z_][A-Za-z0-9_]*` — reuse the bareword char rule, no `-`) and return `Tok::UnitLit(parsed_decimal, suffix)`. The numeric part is parsed as `BigDecimal` (a plain int mantissa like `512` becomes `BigDecimal::from(512)`; underscores stripped as today). Keep the "expected a number" guard for a lone `-`.
- [ ] **Step 4: Run** `cargo test -p mangrove-syntax` → PASS (incl. all existing). `just fmt`.
- [ ] **Step 5: Commit** `feat(syntax): lex unit literals (512Mi, 0.5btc) — reverses D3`.

---

### Task 2: Value::Unit + Type::Brand (model)

**Files:** Modify `crates/mangrove-core/src/value.rs`, `crates/mangrove-syntax/src/ty.rs`.

**Interfaces — Produces:** `Value::Unit { mantissa: BigDecimal, suffix: String }`; `Type::Brand { name: String, inner: Box<Type> }`.

- [ ] **Step 1: Write failing tests** — in `value.rs` add a trivial constructor/equality test for `Value::Unit`; in `ty.rs` a `parse_type("brand int & >= 0")` test (will fail until Task 3 parses `brand`, so gate this test in Task 3 instead — here just add the variant + a construction test):
```rust
    // value.rs
    #[test]
    fn unit_value_constructs() {
        let u = Value::Unit { mantissa: 512.into(), suffix: "Mi".into() };
        assert!(matches!(u, Value::Unit { .. }));
    }
```
- [ ] **Step 2: Implement** — add `Unit { mantissa: BigDecimal, suffix: String }` to `enum Value` (value.rs). Add `Brand { name: String, inner: Box<Type> }` to `enum Type` (ty.rs). Both derive existing traits (`Debug, Clone, PartialEq`).
- [ ] **Step 3: Make the CBOR encoder reject an unresolved unit** — in `mangrove-cbor/src/lib.rs` `encode_into`, the new `Value::Unit` arm must not silently encode; since `encode` returns `Vec<u8>` infallibly, make it `panic!("unresolved unit literal reached the encoder — resolve against a schema first")`. (Resolution always runs before hashing for schema-bound docs; a schemaless unit literal errors earlier at D14, so this panic is unreachable in correct flows — it is a guard, mirror of the no-float invariant.) Add a `# ponytail:` note that this is a guard, not a code path.
- [ ] **Step 4: Run** `cargo test -p mangrove-core -p mangrove-cbor -p mangrove-syntax` → build clean, value test PASS. Fix any non-exhaustive `match` on `Value`/`Type` the new variants introduce (validator/render get their arms in Task 5; for now add minimal arms or `todo!()`-free stubs that the later tasks replace — prefer adding the real arms in Task 5 and a temporary `Value::Unit => unreachable pre-M2b` only where compilation demands).
- [ ] **Step 5: Commit** `feat(core,syntax): Value::Unit and Type::Brand variants`.

---

### Task 3: Parse `unit` declarations and `brand` types

**Files:** Modify `crates/mangrove-syntax/src/parser.rs`, `src/lib.rs`; `Document` gains `unitdefs`.

**Interfaces — Produces:** `mangrove_syntax::UnitDef { name: String, members: Vec<(String, BigInt)> }` (member → base value, evaluated by D13's `coefficient × earlier-member` rule); `Document.unitdefs: Vec<UnitDef>`; `Type::Brand` parsed from a `brand <type>` prefix.

- [ ] **Step 1: Write failing tests** (`parser.rs` tests):
```rust
    #[test]
    fn parses_unit_declaration() {
        let d = parse_document(
            "unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki }\nschema Bytes\nx: 1\n"
        ).unwrap();
        assert_eq!(d.unitdefs.len(), 1);
        let b = &d.unitdefs[0];
        assert_eq!(b.name, "Bytes");
        // Mi resolves to 1024*1024 = 1048576
        assert_eq!(b.members.iter().find(|(n,_)| n=="Mi").unwrap().1, 1048576.into());
    }
    #[test]
    fn brand_type_parses() {
        use crate::ty::Type;
        let t = crate::parse_type("brand int & >= 0").unwrap();
        let Type::Brand { name: _, inner } = t else { panic!() };
        assert!(matches!(*inner, Type::IntRange { .. }));
    }
    #[test]
    fn unit_member_forward_or_unknown_ref_errors() {
        assert!(parse_document("unit U : int { a = 1b }\nschema U\n").is_err()); // unknown member `b`
    }
```
(Note: the `brand` type has no name at the type-expression level — the *name* comes from `type Satoshis = brand …`. Represent the anonymous brand's `name` as the empty string at parse time; M2c/naming can refine. For M2b a `Type::Brand` inside a `type X = brand …` carries `name: X` — set it when the typedef binds. Simplest: `parse_type` yields `Brand { name: "", inner }`, and `parse_typedef` fills `name` from the type's name.)
- [ ] **Step 2: Run** `cargo test -p mangrove-syntax parses_unit` → FAIL.
- [ ] **Step 3: Implement**:
  - `is_keyword_stmt("unit")` branch in `parse_doc` → `parse_unitdef`: `unit Name : int { member = value, … }`. Each `value` is an `Int` token or a `UnitLit(coeff, suffix)` where `suffix` must be an earlier member of *this* unit → base = `coeff × earlier.base` (coeff must be a non-negative integer; error otherwise). Build `members` in declaration order. Unknown/forward member ref → error.
  - In `parse_atom` (type grammar), a leading `Bareword("brand")` → consume, parse the following type, wrap in `Type::Brand { name: String::new(), inner }`.
  - In `parse_typedef`, if the parsed type is `Type::Brand { name, .. }` with empty name, set `name = <typedef name>`.
  - `Document` gains `unitdefs: Vec<UnitDef>`; thread through `parse_doc`.
- [ ] **Step 4: Run** `cargo test -p mangrove-syntax` → PASS. `just fmt`.
- [ ] **Step 5: Commit** `feat(syntax): parse unit declarations and brand types`.

---

### Task 4: TypeEnv units + member resolution

**Files:** Modify `crates/mangrove-typed/src/env.rs`.

**Interfaces — Produces:** `TypeEnv::build(typedefs, unitdefs)` (signature gains unitdefs); `TypeEnv::unit(name) -> Option<&UnitMembers>` where members map suffix→base `BigInt`; resolution helper `resolve_unit(unit_name, mantissa, suffix) -> Result<BigInt, String>` (suffix membership, `mantissa × base` exact-integer check).

- [ ] **Step 1: Write failing tests** (`env.rs`):
```rust
    #[test]
    fn resolve_unit_literal_to_base_int() {
        let units = vec![mangrove_syntax::UnitDef {
            name: "Bytes".into(),
            members: vec![("B".into(),1.into()),("Ki".into(),1024.into()),("Mi".into(),1048576.into())],
        }];
        let env = TypeEnv::build(&[], &units).unwrap();
        assert_eq!(env.resolve_unit("Bytes", &512.into(), "Mi").unwrap(), 536870912.into());
    }
    #[test]
    fn unknown_suffix_errors_with_valid_list() {
        let units = vec![mangrove_syntax::UnitDef { name: "Bytes".into(), members: vec![("Mi".into(),1048576.into())] }];
        let env = TypeEnv::build(&[], &units).unwrap();
        let e = env.resolve_unit("Bytes", &256.into(), "MB").unwrap_err();
        assert!(e.contains("MB") && e.contains("Mi"));
    }
    #[test]
    fn fractional_must_be_exact() {
        let units = vec![mangrove_syntax::UnitDef { name: "Sats".into(), members: vec![("sat".into(),1.into()),("btc".into(),100_000_000.into())] }];
        let env = TypeEnv::build(&[], &units).unwrap();
        assert!(env.resolve_unit("Sats", &bd("0.5"), "btc").is_ok());   // 50_000_000
        assert!(env.resolve_unit("Sats", &bd("0.5"), "sat").is_err());  // 0.5 sat — not integer
    }
    // helpers: bd(s) = BigDecimal::from_str(s).unwrap()
```
- [ ] **Step 2: Run** `cargo test -p mangrove-typed resolve_unit` → FAIL (signature/method).
- [ ] **Step 3: Implement** — `TypeEnv` stores `units: HashMap<String, Vec<(String, BigInt)>>`. `build` takes `unitdefs` and inserts them (dup unit name → error). `resolve_unit`: look up unit (unknown → error); find suffix in members (unknown → `format!("unknown unit `{suffix}`; valid: {members joined}")`); `base = member_base × mantissa`; require exact integer (`(mantissa * base).is_integer()` via BigDecimal; convert to BigInt) else `format!("`{mantissa}{suffix}` is not an exact integer in the base unit")`. Update the existing `TypeEnv::build(&[])` call sites (M2a tests, CLI, conformance) to `build(&[], &[])`.
- [ ] **Step 4: Run** `cargo test -p mangrove-typed` → PASS. `just fmt`.
- [ ] **Step 5: Commit** `feat(typed): unit member resolution in TypeEnv`.

---

### Task 5: Validator arms + resolve pass

**Files:** Modify `crates/mangrove-typed/src/validate.rs`; create `crates/mangrove-typed/src/resolve.rs`; modify `src/lib.rs`.

**Interfaces — Produces:** `mangrove_typed::resolve(value: &Value, ty: &Type, env: &TypeEnv) -> Result<Value, ValidationError>` (units→`Value::Int(base)`; identity elsewhere; recurses); `validate` handles unit-typed fields and `Brand`.

- [ ] **Step 1: Write failing tests** (`resolve.rs` + extend `validate.rs`):
```rust
    // resolve.rs
    #[test]
    fn resolves_unit_field_to_base_int() {
        // schema { size: Bytes }, value { size: 512Mi } → { size: 536870912 }
        // build env with Bytes unit; ty = Map? use a record { size: Named("Bytes") } ...
        // assert resolved body has Value::Int(536870912) at "size"
    }
    // validate.rs
    #[test]
    fn unit_value_validates_against_unit_type() { /* 512Mi vs Bytes field → ok */ }
    #[test]
    fn wrong_unit_suffix_errors() { /* 1core vs Bytes field → error */ }
    #[test]
    fn brand_validates_against_inner() { /* 21000 vs `brand int & >= 0` → ok; -1 → error */ }
```
(Flesh these with concrete `Value`/`Type` built via `parse_type` + `TypeEnv` as in M2a's validator tests; a unit type is referenced by name, so use `TypeEnv::build(&[], &units)` and `Type::Named("Bytes")`.)
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement**:
  - `validate` (validate.rs): when the type resolves (possibly via `Named`) to a unit type, accept `Value::Unit{mantissa,suffix}` (call `env.resolve_unit`; on `Err` push a §12 error with the message as `failed`) and `Value::Int` (a bare base-unit integer, then apply the unit's `int & …` refinement); any other value kind → mismatch. For `Type::Brand { inner, .. }` → validate against `inner` (auto-construction: a bare literal validating against `inner` is accepted as the brand). Add a `Value::Unit` arm to `render`.
  - `resolve` (resolve.rs): mirror the validate walk but **produce a new `Value`**: a `Value::Unit` in a unit field → `Value::Int(env.resolve_unit(...)?)`; records/maps/lists recurse; everything else clones. A schemaless call (no schema) is not made — `resolve` is only invoked when a schema is bound.
  - `lib.rs`: `pub use resolve::resolve;`.
- [ ] **Step 4: Run** `cargo test -p mangrove-typed` → PASS.
- [ ] **Step 5: Commit** `feat(typed): validate units/brands and add the resolve pass`.

---

### Task 6: Content address = resolved form (CLI + canonical)

**Files:** Modify `crates/mangrove-cli/src/main.rs`.

**Interfaces — Produces:** `mangrove hash <file>` resolves a schema-bound document (units→ints) before hashing; a schemaless document hashes as in M1; a schemaless document containing a unit literal errors (D14). `mangrove check` resolves+validates.

- [ ] **Step 1: Write failing tests** (`crates/mangrove-cli/tests/cli.rs`):
```rust
#[test]
fn hash_resolves_units_so_512mi_equals_536870912() {
    let a = std::env::temp_dir().join("m2b_a.mang");
    let b = std::env::temp_dir().join("m2b_b.mang");
    let unit = "unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki }\ntype D = { size: Bytes }\nschema D\n";
    std::fs::write(&a, format!("{unit}size: 512Mi\n")).unwrap();
    std::fs::write(&b, format!("{unit}size: 536870912\n")).unwrap();
    let h = |p| String::from_utf8(Command::new(env!("CARGO_BIN_EXE_mangrove")).arg("hash").arg(p).output().unwrap().stdout).unwrap();
    assert_eq!(h(&a), h(&b)); // §4.5: same value
}
#[test]
fn schemaless_unit_literal_errors() {
    let p = std::env::temp_dir().join("m2b_bare.mang");
    std::fs::write(&p, "x: 512Mi\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove")).arg("hash").arg(&p).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}
```
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `cmd_hash`: parse_document → build TypeEnv(typedefs, unitdefs) → if a schema is bound, `resolve(body, schema_ty, env)` (error → exit 1) and hash the resolved value; if no schema, hash the body as-is **unless** it contains an unresolved `Value::Unit` (walk → error "unit literal requires a schema", exit 1). `cmd_check` already validates; ensure it builds the env with unitdefs.
- [ ] **Step 4: Run** `cargo test -p mangrove-cli` → PASS, and `cargo test -p mangrove-conformance` (L0 hashes unchanged).
- [ ] **Step 5: Commit** `feat(cli): content address is the schema-resolved form (units→base int)`.

---

### Task 7: Conformance — L1 unit/brand vectors

**Files:** Modify `crates/mangrove-conformance/{src/lib.rs,tests/corpus.rs}`; add `tests/conformance/l1/` vectors.

- [ ] **Step 1:** Extend `run_check_vector`/add `run_resolved_hash_vector` if a unit doc needs a hash check; reuse `run_check_vector` for error/ok vectors. Build the `TypeEnv` with `unitdefs` from the parsed document.
- [ ] **Step 2: Add vectors** under `tests/conformance/l1/`: `unit_ok` (512Mi in a Bytes field → `ok`), `unit_unknown_suffix` (256MB → error listing valid units), `unit_fractional_inexact` (0.5sat → error), `brand_ok` (bare literal into a brand field → `ok`), `brand_range` (negative into `brand int & >= 0` → error). Generate `.expected` by running `mangrove check` and pinning (eyeball each).
- [ ] **Step 3: Run** `cargo test -p mangrove-conformance` → PASS (L0 + M2a L1 + new).
- [ ] **Step 4: Full gate** `just ci` → green. Push `main`, verify CI.
- [ ] **Step 5: Commit** `feat(conformance): L1 unit/brand vectors`.

---

## Self-Review

**Spec coverage:** lexer unit literals (D3 reversed) → T1; Value::Unit/Type::Brand → T2; unit decls + brand parse (D13) → T3; member resolution → T4; validation + resolve pass → T5; resolved content address (D12) + D14 → T6; conformance → T7. ✓

**Hash-stability guard:** every task runs the L0 conformance corpus; D12 must leave all M1 hashes unchanged (schemaless docs resolve to themselves).

**Type consistency:** `Value::Unit{mantissa,suffix}` (T2) flows lexer→parser→validate/resolve; `TypeEnv::build(typedefs, unitdefs)` (T4) updates all M2a call sites; `resolve` (T5) consumed by CLI hash (T6); `UnitDef{name,members}` (T3) consumed by TypeEnv (T4).
