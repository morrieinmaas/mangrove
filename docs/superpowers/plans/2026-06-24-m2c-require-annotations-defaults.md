# M2c — require, annotations & defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline). Steps use `- [ ]`. Design: `../specs/2026-06-24-m2c-require-annotations-defaults-design.md`. Decisions D15–D18.

**Goal:** Complete L1 — field defaults (`| *value`) materialized into the canonical form, `@doc`/`@message`/`@deprecated` annotations, and cross-field `require` predicates. `match` deferred to M4 (D15).

**Tech Stack:** Rust 2024; reuses existing crates. No new deps.

## Global Constraints

- Edition 2024, Apache-2.0, workspace inheritance, `unsafe_code = "forbid"`, clippy `-D warnings`. Gate per task: `just ci` green; clippy via `rtk proxy cargo clippy`.
- Commit directly to `main`. No M1/M2a/M2b hash may change except where a **default is now materialized** (that is the intended D18 change; assert L0 + non-defaulted vectors are unchanged).
- TDD: failing test first.

Ordered by increasing complexity: **defaults → annotations → require**.

---

### Task 1: Lexer tokens for M2c

**Files:** `crates/mangrove-syntax/src/lexer.rs`.

**Produces:** `Tok::Star` (`*`), `Tok::At` (`@`), `Tok::Dot` (`.`), `Tok::EqEq` (`==`), `Tok::Bang` (`!`), `Tok::AmpAmp` (`&&`), `Tok::PipePipe` (`||`). (`!=` is `Tok::Ne`.) `Pipe`/`Amp`/`Eq`/`Match` already exist; make `&&`/`||`/`==`/`!=` greedy two-char before the one-char forms.

- [ ] **Step 1: Failing test** — token stream for `* @ . == != ! && ||`.
- [ ] **Step 2: Implement** — add variants; in `run()`: `'*'`→Star, `'@'`→At, `'.'`→Dot (NOTE: numbers consume their own `.` in `lex_number`, so a standalone `.` reaching `run()` is always a path dot), `'!'`→ if next `=` then `Ne` else `Bang`, `'&'`→ if next `&` then `AmpAmp` else `Amp`, `'|'`→ if next `|` then `PipePipe` else `Pipe`, `'='`→ if next `=` then `EqEq` elif next `~` then `Match` else `Eq`.
- [ ] **Step 3: Run** `cargo test -p mangrove-syntax` → PASS (all existing too). `just fmt`.
- [ ] **Step 4: Commit** `feat(syntax): lex M2c tokens (* @ . == != ! && ||)`.

---

### Task 2: Defaults — parse `field: T | *value`

**Files:** `crates/mangrove-syntax/src/{ty.rs,parser.rs}`.

**Produces:** `FieldDef.default: Option<Value>`. The type-expr union loop stops at `| *` (leaving it for the field parser); the field parser consumes `| *` then parses a default `Value` (reusing `parse_value`).

- [ ] **Step 1: Failing tests** (`ty.rs`): `parse_type("{ ns: str | *\"d\", n: int | *1, f?: bool }")` → fields with `default == Some(Value::Str("d"))`, `Some(Value::Int(1))`, and `f` default `None` optional `true`. Also `{ a: str | b }` (real union, no star) still parses `a: str | b` as a union-typed field.
- [ ] **Step 2: Implement** — add `default: Option<Value>` to `FieldDef`. In `parse_type_expr`'s union loop: `while self.check(&Tok::Pipe) && self.peek_at(1) != Some(Tok::Star-equivalent)` — i.e. don't consume `|` when the next token is `Star`. (Add a tokens-based lookahead: peek the token at `pos+1`.) In `parse_record_or_map`'s field loop: after parsing the field type, `if Pipe followed by Star { consume both; default = Some(parse_value(0)?) }`. Update all `FieldDef { … }` literals (env.rs test, anywhere) to include `default: None`.
- [ ] **Step 3: Run** `cargo test -p mangrove-syntax` → PASS. `just fmt`.
- [ ] **Step 4: Commit** `feat(syntax): parse field defaults (| *value)`.

---

### Task 3: Defaults — validate + materialize

**Files:** `crates/mangrove-typed/src/{validate.rs,resolve.rs}`; conformance.

**Produces:** absent defaulted field is valid (not "missing"); default type-checked at load-ish (validate the default against the field type — do it in the record arm when materializing, or once at build); `resolve` fills an absent defaulted field with its default → materialized canonical form (D18). A bare optional absent field stays absent.

- [ ] **Step 1: Failing tests** (`validate.rs` + `resolve.rs`):
  - validate: a record `{ n: int | *1 }` with `n` absent → no error.
  - validate: default itself ill-typed (`{ n: int & >= 1 | *0 }`) → error (the default `0` violates `>= 1`).
  - resolve: `{ n: int | *1 }` with `n` absent → resolved map has `n: 1`.
  - resolve: `{ n?: bool }` with `n` absent → resolved map has no `n`.
- [ ] **Step 2: Implement** — validate Record arm: `None if f.optional => {}`; `None if f.default.is_some()` → validate the default against `f.ty` (so an ill-typed default surfaces) then OK; else the missing-required error. resolve Record arm: when a defaulted field is absent, insert `resolve_at(default, &f.ty, …)`. Keep present fields as-is.
- [ ] **Step 3: Run** `cargo test -p mangrove-typed` → PASS.
- [ ] **Step 4: Conformance** — add `default_materialized.mang` (omits a defaulted field) + a sibling writing it explicitly, assert equal hashes via a small harness addition or a CLI test; add to L1 vectors. Confirm L0 + existing L1 vectors unchanged.
- [ ] **Step 5: Commit** `feat(typed): validate and materialize field defaults (§7 step 3)`.

---

### Task 4: Annotations — parse `@doc/@message/@deprecated`

**Files:** `crates/mangrove-syntax/src/{ty.rs,parser.rs}`.

**Produces:** `Annotation { name: String, arg: Option<String> }`; typedef annotations returned alongside the type (`parse_typedef -> (String, Type, Vec<Annotation>)` or a struct); `FieldDef.annotations: Vec<Annotation>`.

- [ ] **Step 1: Failing tests** — `type Port = int @doc("p") @message("m")` captures two annotations; `{ image: str @deprecated("use x") }` field carries one.
- [ ] **Step 2: Implement** — `parse_annotations` reads `{ @ ident ( str ) }*`. In `parse_typedef`, after the type, parse trailing annotations. In the record field parser, after the type/default, parse trailing annotations into `FieldDef.annotations`. Thread typedef annotations into `Document` (e.g. `typedefs: Vec<(String, Type, Vec<Annotation>)>` or a `TypeDef` struct — update call sites).
- [ ] **Step 3: Run** `cargo test -p mangrove-syntax` → PASS. `just fmt`.
- [ ] **Step 4: Commit** `feat(syntax): parse @doc/@message/@deprecated annotations`.

---

### Task 5: Annotations — wire @message into errors, @deprecated advisories

**Files:** `crates/mangrove-typed/*`, `crates/mangrove-cli/src/main.rs`.

**Produces:** `TypeEnv` stores per-named-type annotations; validating against a named type that has `@message` sets the error `message`; `mangrove-typed` returns deprecation advisories (e.g. `validate_doc` returning `(errors, warnings)` or a separate `deprecations(doc, env)` walk); CLI prints advisories.

- [ ] **Step 1: Failing tests** — validating `42000` against a named `Port` (with `@message("…")`, range 1–65535 fails for 70000) sets `error.message == Some("…")`. A present `@deprecated` field yields a warning.
- [ ] **Step 2: Implement** — `TypeEnv::build` takes typedef annotations; store `messages: HashMap<String,String>`. In `validate`'s `Named` arm, if the named type fails and has a message, set it on the produced errors (post-process the sub-errors’ `message`). Add a `deprecations(value, ty, env) -> Vec<String>` walk collecting present fields whose `FieldDef`/type is `@deprecated`. CLI `check` prints advisories to stderr (exit unaffected).
- [ ] **Step 3: Run** `cargo test -p mangrove-typed -p mangrove-cli` → PASS.
- [ ] **Step 4: Commit** `feat(typed,cli): surface @message in errors and @deprecated advisories`.

---

### Task 6: require — predicate AST + parser

**Files:** `crates/mangrove-syntax/src/{ty.rs,parser.rs}`.

**Produces:** `Pred` AST (`Or/And/Not/Cmp/Path/Lit/Len`), `Require { pred: Pred, message: Option<String> }`, `Type::Record.requires: Vec<Require>`. A `require:` line inside a record type parses a predicate (grammar in spec §2) + optional trailing `@message`.

- [ ] **Step 1: Failing tests** — parse `{ a: int, b: int, require: a <= b }` → one require; `require: tls == false || len(certs) >= 1 @message("m")` → pred + message; precedence `||` looser than `&&` looser than comparison.
- [ ] **Step 2: Implement** — in the record field loop, recognize the bareword `require` followed by `:` as a require clause (not a field): parse the predicate (recursive-descent per spec grammar; `len` is `Bareword("len") (`  path `)`), then optional `@message`. Add `Pred`/`Require` to `ty.rs`; `Type::Record` gains `requires`. Update Record literals/matches (validate render, env collect_refs ignore requires, etc.).
- [ ] **Step 3: Run** `cargo test -p mangrove-syntax` → PASS. `just fmt`.
- [ ] **Step 4: Commit** `feat(syntax): parse require predicates`.

---

### Task 7: require — evaluate against values

**Files:** `crates/mangrove-typed/src/{validate.rs, (new) predicate.rs}`; conformance.

**Produces:** `eval_pred(pred, record_value) -> Result<bool, String>`; the validator evaluates each `require` after a record's fields validate; a `false` (or eval error) → §12 error with the `@message`.

- [ ] **Step 1: Failing tests** — `{ a: int, b: int, require: a <= b }` with `{a:1,b:2}` ok; `{a:5,b:2}` → error (message if present); `len(certs) >= 1` over an empty vs non-empty list; cross-kind compare (`"x" == 1`) → error not panic; path to a missing field → error not panic.
- [ ] **Step 2: Implement** — `predicate.rs`: `eval_pred` walks the `Pred`, resolving `Path` against the record `Value::Map` (kind-aware comparisons over Int/Decimal/Str/Bool; `len` over list/map/str; boolean ops). In `validate`'s Record arm, after fields, evaluate each require; on `false`/`Err` push a §12 error (`failed: "require"`, message from the clause).
- [ ] **Step 3: Run** `cargo test -p mangrove-typed` → PASS.
- [ ] **Step 4: Conformance** — L1 vectors: `require_ok`, `require_fail` (with @message), `require_len`. Confirm all prior vectors unchanged.
- [ ] **Step 5: Full gate** `just ci` → green; push; verify CI.
- [ ] **Step 6: Commit** `feat(typed): evaluate require predicates against values`.

---

## Self-Review

**Spec coverage:** tokens → T1; defaults parse → T2, validate+materialize (D18) → T3; annotations parse → T4, @message/@deprecated wiring (D17) → T5; require parse → T6, eval (D16) → T7. `match` correctly absent (D15). ✓

**Hash-stability:** only intended change is default materialization (D18); L0 and non-defaulted L1 vectors must stay identical — asserted in T3/T7 conformance steps.

**Consistency:** `FieldDef` gains `default` (T2) and `annotations` (T4) — every `FieldDef{…}` literal across the workspace updated in the task that adds the field. `Type::Record` gains `requires` (T6) — all Record literals/matches updated then.
