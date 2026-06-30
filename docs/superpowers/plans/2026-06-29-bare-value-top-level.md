# Bare-Value Top-Level Documents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow a Mangrove document body to be a single bare value (list, scalar, string, etc.) rather than requiring `key: value` bindings — unblocking multi-doc YAML round-tripping and making the axiom "every document reduces to a single canonical value" concrete.

**Architecture:** Add a disambiguation helper to both the legacy parser (`parse_doc`) and the CST parser (`parse_document`) that peeks at the first significant body token and decides: if it clearly starts a value expression (not a binding or spread), parse it as a bare top-level value instead of accumulating bindings. Both parsers must lower identically so the equivalence oracle can cover them. Lowering in `lower.rs` grows a new `BARE_VALUE` node kind handled in the `lower()` dispatch.

**Tech Stack:** Rust 2024, `rowan` (green tree / CST), `mangrove-syntax` crate, `mangrove-canonical` for hash, `mangrove-core::Value`.

## Global Constraints

- Rust 2024 edition; NO `unsafe`.
- Conventional Commits; NO `Co-Authored-By`/AI attribution in commit messages.
- Commit directly to `main` (trunk-based dev); do NOT push.
- TDD first (RED→GREEN). `cargo fmt` + `just ci` green before commit.
- Commit only — NEVER `git add .superpowers/`.
- Clippy via `rtk proxy cargo clippy --workspace` if the hook misreports; `just ci` is source of truth.
- `{`-leading bodies: keep existing behaviour unchanged. The bare-value path is ONLY for `[`, scalars, `unset`, `match`, and a bareword NOT followed by `:` / `+=` / `{` (a reference). If `{` is the first body token, fall through to the existing binding-accumulation path (which will parse it as a keyed statement or error as it currently does).

---

## Disambiguation Rule (read this before touching any code)

After consuming all declarations (`use`, `type`, `unit`, `params`, `fn`, `schema`), the body begins. The body token is the **first non-sep, non-EOF token** that is not part of a declaration.

**Bare-value path** — first body token is ANY of:
- `[` (L_BRACKET / `Tok::LBracket`) — unambiguous list
- A string literal (`Tok::Str`) — could never be a key here because there's no `:` following it at the body level (note: `Str` followed by `:` IS a binding; so we peek TWO tokens)
- An integer, decimal, bool, bytes, unit-literal, interp-string scalar
- `unset` (bareword where text == "unset")
- `match` (bareword where text == "match")
- A bareword NOT followed by `:`, `+=`, or `{` — this is a bare reference

**Binding-accumulation path** (existing behaviour, unchanged) — first body token is:
- A bareword followed by `:` (`Tok::Colon`) — keyed bind
- A bareword followed by `+=` (`Tok::PlusEq`) — append stmt
- A bareword followed by `{` (`Tok::LBrace`) — list-op block
- A string literal followed by `:` — keyed bind with quoted key
- `...` (spread)
- `{` (LBrace) — fall through to existing path (do not change this)

**Empty body** (EOF after declarations) — stays `Value::Map({})` as today.

The peek is 2-token lookahead: (token[0], token[1]). Both parsers have utilities for this already (`next_is` / `nth_sig`).

---

## File Map

- **Modify:** `crates/mangrove-syntax/src/parser.rs` — `parse_doc`, the body-dispatch `else` arm (lines ~430–473)
- **Modify:** `crates/mangrove-syntax/src/cst/parse.rs` — `parse_document` function (lines ~154–188), add `parse_bare_value_body` helper
- **Modify:** `crates/mangrove-syntax/src/cst/kind.rs` — add `BARE_VALUE` to `SyntaxKind` enum and `ALL` array (before `__LAST`)
- **Modify:** `crates/mangrove-syntax/src/cst/lower.rs` — `lower()` dispatch, handle `BARE_VALUE` node
- **Modify:** `crates/mangrove-syntax/src/cst/tests.rs` — oracle tests, losslessness tests, multi-doc test

---

## Task 1: Red tests — write all failing tests

**Files:**
- Modify: `crates/mangrove-syntax/src/parser.rs` (add tests in the `#[cfg(test)]` block, ~line 1575+)
- Modify: `crates/mangrove-syntax/src/cst/tests.rs` (add oracle + losslessness tests)

**Interfaces:**
- Consumes: nothing new — uses existing `parse_document`, `parse`, `assert_document_equivalent`, `assert_hash_equivalent`, `parse_cst`
- Produces: nothing (test-only)

- [ ] **Step 1: Add failing tests to `parser.rs`**

Append this block inside the `#[cfg(test)] mod tests { … }` at the bottom of `crates/mangrove-syntax/src/parser.rs` (before the closing `}`):

```rust
    // ---- bare-value top-level documents ----

    #[test]
    fn bare_list_document() {
        let d = parse_document("[ 1, 2, 3 ]\n").unwrap();
        assert!(d.stmts.is_empty(), "bare-value doc has no stmts");
        assert_eq!(
            d.body,
            Value::List(vec![
                Value::Int(1.into()),
                Value::Int(2.into()),
                Value::Int(3.into()),
            ])
        );
    }

    #[test]
    fn bare_int_document() {
        let d = parse_document("42\n").unwrap();
        assert!(d.stmts.is_empty());
        assert_eq!(d.body, Value::Int(42.into()));
    }

    #[test]
    fn bare_string_document() {
        let d = parse_document("\"hello\"\n").unwrap();
        assert!(d.stmts.is_empty());
        assert_eq!(d.body, Value::Str("hello".into()));
    }

    #[test]
    fn bare_bool_document() {
        let d = parse_document("true\n").unwrap();
        assert!(d.stmts.is_empty());
        assert_eq!(d.body, Value::Bool(true));
    }

    #[test]
    fn bare_value_parse_helper() {
        // `parse` (the hash entrypoint) must also work
        let v = parse("[ 10, 20 ]\n").unwrap();
        assert_eq!(v, Value::List(vec![Value::Int(10.into()), Value::Int(20.into())]));
    }

    #[test]
    fn bare_ref_document() {
        // a bareword not followed by : is a reference
        let d = parse_document("myref\n").unwrap();
        assert!(d.stmts.is_empty());
        assert_eq!(d.body, Value::Ref("myref".into()));
    }

    #[test]
    fn bare_value_with_declarations() {
        use crate::ty::Type;
        // type defs + schema + bare list body
        let src = "type Port = int & >= 1 & <= 65535\nschema Port\n[ 8443, 9090 ]\n";
        let d = parse_document(src).unwrap();
        assert_eq!(d.typedefs.len(), 1);
        assert_eq!(d.schema.as_deref(), Some("Port"));
        assert_eq!(
            d.body,
            Value::List(vec![Value::Int(8443.into()), Value::Int(9090.into())])
        );
    }

    #[test]
    fn quoted_key_is_still_a_binding_not_bare_value() {
        // A string followed by `:` must remain a binding, not a bare value.
        let d = parse_document("\"my-key\": 42\n").unwrap();
        let Value::Map(m) = &d.body else { panic!("expected map") };
        assert_eq!(m.get("my-key"), Some(&Value::Int(42.into())));
    }

    #[test]
    fn bare_value_empty_list() {
        let d = parse_document("[]\n").unwrap();
        assert_eq!(d.body, Value::List(vec![]));
    }
```

- [ ] **Step 2: Add failing tests to `cst/tests.rs`**

Append this block before the final `}` of `crates/mangrove-syntax/src/cst/tests.rs`:

```rust
// ---- bare-value top-level documents ----

#[test]
fn oracle_bare_list_document() {
    assert_document_equivalent("[ 1, 2, 3 ]\n");
}

#[test]
fn oracle_bare_int_document() {
    assert_document_equivalent("42\n");
}

#[test]
fn oracle_bare_string_document() {
    assert_document_equivalent("\"hello\"\n");
}

#[test]
fn oracle_bare_bool_document() {
    assert_document_equivalent("true\n");
}

#[test]
fn oracle_bare_ref_document() {
    assert_document_equivalent("myref\n");
}

#[test]
fn oracle_bare_empty_list() {
    assert_document_equivalent("[]\n");
}

#[test]
fn oracle_bare_value_with_declarations() {
    assert_document_equivalent(
        "type Port = int & >= 1 & <= 65535\nschema Port\n[ 8443, 9090 ]\n",
    );
}

#[test]
fn bare_value_cst_losslessness() {
    for src in [
        "[ 1, 2, 3 ]\n",
        "42\n",
        "\"hello\"\n",
        "true\n",
        "[]\n",
        "type Port = int & >= 1 & <= 65535\nschema Port\n[ 8443, 9090 ]\n",
    ] {
        let node = super::parse::parse_cst(src).syntax();
        assert_eq!(
            node.text().to_string(),
            src,
            "bare-value document must round-trip losslessly: {src:?}"
        );
    }
}

#[test]
fn oracle_bare_value_hash() {
    // The hash of a bare-list document matches the hash of Value::List directly
    assert_hash_equivalent("[ 1, 2, 3 ]\n");
    assert_hash_equivalent("[]\n");
    assert_hash_equivalent("42\n");
    assert_hash_equivalent("\"hello\"\n");
}

#[test]
fn bare_value_cst_node_kind() {
    // The CST must emit a BARE_VALUE node as a direct child of DOCUMENT
    use super::kind::SyntaxKind;
    let p = super::parse::parse_cst("[ 1, 2, 3 ]\n");
    let root = p.syntax();
    let bare_val = root
        .children()
        .find(|n| n.kind() == SyntaxKind::BARE_VALUE);
    assert!(
        bare_val.is_some(),
        "expected a BARE_VALUE child of DOCUMENT for a bare list"
    );
    // No BINDING children — this is a bare doc, not a binding doc
    let bindings = root
        .children()
        .filter(|n| n.kind() == SyntaxKind::BINDING)
        .count();
    assert_eq!(bindings, 0);
}
```

- [ ] **Step 3: Run tests to confirm they all fail (RED)**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test -p mangrove-syntax 2>&1 | grep -E "^(FAILED|error|test .* FAILED)" | head -40
```

Expected: multiple test failures — `bare_list_document`, `oracle_bare_list_document`, `bare_value_cst_node_kind`, etc. These should all be compile errors or assertion failures, NOT panics on existing tests.

- [ ] **Step 4: Commit red tests**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/parser.rs crates/mangrove-syntax/src/cst/tests.rs
git commit -m "test(syntax): RED — bare-value top-level document tests"
```

---

## Task 2: Add `BARE_VALUE` to `SyntaxKind`

**Files:**
- Modify: `crates/mangrove-syntax/src/cst/kind.rs`

**Interfaces:**
- Produces: `SyntaxKind::BARE_VALUE` available for use in `parse.rs` and `lower.rs`

- [ ] **Step 1: Add `BARE_VALUE` variant to the enum**

In `crates/mangrove-syntax/src/cst/kind.rs`, find the `BINDING,` line (currently the last body-node entry before the type nodes) and add `BARE_VALUE` right after it:

```rust
    BINDING,
    BARE_VALUE,   // NEW: a document whose body is a single bare value
    // types
```

- [ ] **Step 2: Add `BARE_VALUE` to the `ALL` array**

In `kind.rs`, find `SyntaxKind::BINDING,` in the `ALL` array and add `SyntaxKind::BARE_VALUE,` immediately after:

```rust
        SyntaxKind::BINDING,
        SyntaxKind::BARE_VALUE,
        SyntaxKind::TYPE_PRIMITIVE,
```

- [ ] **Step 3: Verify the `__LAST` invariant compiles and passes**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test -p mangrove-syntax syntaxkind_all_matches_discriminants 2>&1
```

Expected: `test syntaxkind_all_matches_discriminants ... ok` (the test verifies `ALL.len() == __LAST as usize`).

- [ ] **Step 4: Commit**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/cst/kind.rs
git commit -m "feat(syntax): add BARE_VALUE SyntaxKind for bare-value document bodies"
```

---

## Task 3: Legacy parser — `parse_doc` disambiguation

**Files:**
- Modify: `crates/mangrove-syntax/src/parser.rs` — `parse_doc` method (~line 353)

**Interfaces:**
- Consumes: `parse_value(0)` already exists on `Parser`; `next_is(&Tok::X)` already exists
- Produces: `Document { stmts: vec![], body: <parsed value> }` when bare-value path taken

The key insight: after all declarations are consumed and `skip_seps` is called, we inspect `peek()` and `tokens[pos+1]` to decide. The existing `else` arm at ~line 430 (which tries to parse a keyed statement) must be guarded by a new check: if the first body token is NOT the start of a binding, parse it as a value instead.

- [ ] **Step 1: Add a `is_bare_value_start` helper method to `Parser`**

Find the `is_keyword_stmt` method in `parser.rs` and add this method directly after it (around line 702):

```rust
    /// True if the current token starts a bare-value body (not a keyed binding or spread).
    /// Requires that `skip_seps` has already been called so we're at the first body token.
    ///
    /// Bare-value: `[`, scalars (int/dec/bool/bytes/unit/interp/str), `unset`, `match`,
    /// or a bareword NOT followed by `:` / `+=` / `{`.
    ///
    /// `{`-leading bodies are NOT bare-value — they fall through to existing binding logic.
    fn is_bare_value_start(&self) -> bool {
        match &self.peek().tok {
            // List literal — unambiguously a bare value
            Tok::LBracket => true,
            // All scalar tokens — never a key on their own
            Tok::Int(_)
            | Tok::Decimal(_)
            | Tok::UnitLit(_, _)
            | Tok::Bool(_)
            | Tok::Bytes(_)
            | Tok::InterpStr(_) => true,
            // String literal: bare value ONLY if NOT followed by `:`
            Tok::Str(_) => !self.next_is(&Tok::Colon),
            // `unset` or `match` barewords are values, not keys
            Tok::Bareword(b) if b == "unset" || b == "match" => true,
            // Other barewords: bare value only if next token is NOT `:`, `+=`, or `{`
            Tok::Bareword(_) => {
                !self.next_is(&Tok::Colon)
                    && !self.next_is(&Tok::PlusEq)
                    && !self.next_is(&Tok::LBrace)
            }
            // Everything else (including `{`) — not a bare-value start
            _ => false,
        }
    }
```

- [ ] **Step 2: Modify `parse_doc`'s body-dispatch `else` arm**

In `parse_doc`, find the `else {` arm that handles keyed body statements (around line 430). The arm begins with:

```rust
            } else {
                // A keyed body statement: `k: v`, `k += [..]`, or `k { ops }`.
                let key = match self.peek().tok.clone() {
```

Replace the entire `else { … }` block (from `} else {` through the closing `}` of the match's body, before the separator logic at line 475) with this:

```rust
            } else if self.is_bare_value_start() {
                // Bare-value body: the document is a single value, not a record of bindings.
                let value = self.parse_value(0)?;
                // Consume the trailing newline/sep so the loop terminates cleanly.
                let had_sep = self.at_sep();
                self.skip_seps();
                if !self.at_eof() {
                    return Err(self.error(
                        "unexpected token after bare-value document body".into(),
                    ));
                }
                return Ok(Document {
                    uses,
                    typedefs,
                    unitdefs,
                    schema,
                    schema_narrow,
                    params,
                    fns,
                    stmts: vec![],
                    body: value,
                });
            } else {
                // A keyed body statement: `k: v`, `k += [..]`, or `k { ops }`.
                let key = match self.peek().tok.clone() {
                    Tok::Bareword(n) => {
                        self.advance();
                        n
                    }
                    Tok::Str(n) => {
                        self.advance();
                        n
                    }
                    other => return Err(self.error(format!("expected a key, found {other:?}"))),
                };
                match self.peek().tok {
                    Tok::Colon => {
                        self.advance();
                        let value = self.parse_value(0)?;
                        if !matches!(value, Value::Unset) {
                            if body.contains_key(&key) {
                                return Err(self.error(format!("duplicate key {key:?}")));
                            }
                            body.insert(key.clone(), value.clone());
                        }
                        stmts.push(Stmt::Bind(key, value));
                    }
                    Tok::PlusEq => {
                        self.advance();
                        let value = self.parse_value(0)?;
                        stmts.push(Stmt::Append(key, value));
                    }
                    Tok::LBrace => {
                        let items = self.parse_list_op_block()?;
                        stmts.push(Stmt::ListOp(key, items));
                    }
                    ref other => {
                        return Err(self.error(format!(
                            "expected ':', '+=', or '{{' after key, found {other:?}"
                        )));
                    }
                }
            }
```

The `had_sep` / `skip_seps` / separator-check block that follows (around lines 475–479) should remain unchanged for the binding path. Because the bare-value path returns early, it doesn't fall through to those lines.

- [ ] **Step 3: Run legacy parser tests to verify RED→GREEN**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test -p mangrove-syntax --lib parser 2>&1 | tail -30
```

Expected: all `bare_*` tests in `parser.rs` pass (`ok`). All pre-existing tests still pass. Zero failures.

- [ ] **Step 4: Commit**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/parser.rs
git commit -m "feat(syntax): legacy parser — bare-value top-level document body"
```

---

## Task 4: CST parser — `parse_document` bare-value path

**Files:**
- Modify: `crates/mangrove-syntax/src/cst/parse.rs`

**Interfaces:**
- Consumes: `SyntaxKind::BARE_VALUE` (from Task 2), `parse_atom` (already exists), `nth_sig` helper
- Produces: a `BARE_VALUE` node wrapping the value expression emitted as a direct child of `DOCUMENT`

The CST `parse_document` function must apply the same disambiguation. After all declarations are consumed, if the next significant token starts a bare value, emit a `BARE_VALUE` node containing the atom (using `parse_atom`). The lossless tree text must still equal the source.

- [ ] **Step 1: Add `is_bare_value_start_cst` helper**

Add this free function near the other lookahead helpers in `cst/parse.rs` (after `lookahead_is_lbrace`, around line 219):

```rust
/// True if the current significant token starts a bare-value body.
///
/// Mirrors `Parser::is_bare_value_start` in parser.rs — same rule applied to
/// `SyntaxKind` instead of `Tok`. `{`-leading bodies are NOT bare-value.
fn is_bare_value_start_cst(p: &Parser) -> bool {
    match p.current() {
        // List literal — unambiguously a bare value
        SyntaxKind::L_BRACKET => true,
        // All scalar tokens — never a key
        SyntaxKind::INT
        | SyntaxKind::DECIMAL
        | SyntaxKind::UNIT_LIT
        | SyntaxKind::BOOL
        | SyntaxKind::BYTES
        | SyntaxKind::INTERP_STR => true,
        // String: bare value only if NOT followed by COLON
        SyntaxKind::STR => nth_sig(p, 1) != SyntaxKind::COLON,
        // Bareword: check the text for `unset`/`match`, or check that next token is not `:` / `+=` / `{`
        SyntaxKind::BAREWORD => {
            let text = current_bareword_text(p);
            match text.as_deref() {
                Some("unset") | Some("match") => true,
                _ => {
                    nth_sig(p, 1) != SyntaxKind::COLON
                        && nth_sig(p, 1) != SyntaxKind::PLUS_EQ
                        && nth_sig(p, 1) != SyntaxKind::L_BRACE
                }
            }
        }
        // Everything else (including L_BRACE) — not a bare-value start
        _ => false,
    }
}
```

- [ ] **Step 2: Add `parse_bare_value_body` helper**

Add this function after `parse_spread` in `cst/parse.rs` (around line 518):

```rust
/// A bare-value document body: a single value expression at the top level.
/// Wraps the value in a BARE_VALUE node so `lower.rs` can identify it.
fn parse_bare_value_body(p: &mut Parser) {
    p.start(SyntaxKind::BARE_VALUE);
    parse_atom(p, false, 0);
    // Consume trailing newline if present (keeps the tree lossless)
    if p.current() == SyntaxKind::NEWLINE {
        p.bump();
    }
    p.finish();
}
```

- [ ] **Step 3: Wire into `parse_document`**

In `parse_document`, replace the `else` arm of the body dispatch (after the spread check at line ~175) with a check for bare-value start. Find the section:

```rust
        } else if p.current() == SyntaxKind::DOT_DOT_DOT {
            parse_spread(p);
        } else {
            let second = nth_sig(p, 1);
            if (p.current() == SyntaxKind::BAREWORD || p.current() == SyntaxKind::STR)
                && (second == SyntaxKind::PLUS_EQ || second == SyntaxKind::L_BRACE)
            {
                parse_list_op_item(p);
            } else {
                parse_binding(p);
            }
        }
```

Replace it with:

```rust
        } else if p.current() == SyntaxKind::DOT_DOT_DOT {
            parse_spread(p);
        } else if is_bare_value_start_cst(p) {
            parse_bare_value_body(p);
            // After the bare-value body, only EOF is valid — any remaining tokens
            // will produce errors on the next loop iteration naturally (they'll hit
            // the error_and_recover path). We break here to mirror parse_doc's early-return.
            break;
        } else {
            let second = nth_sig(p, 1);
            if (p.current() == SyntaxKind::BAREWORD || p.current() == SyntaxKind::STR)
                && (second == SyntaxKind::PLUS_EQ || second == SyntaxKind::L_BRACE)
            {
                parse_list_op_item(p);
            } else {
                parse_binding(p);
            }
        }
```

- [ ] **Step 4: Run CST parse tests to verify losslessness and node kind**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test -p mangrove-syntax --lib cst::tests::bare_value_cst 2>&1
```

Expected: `bare_value_cst_losslessness ... ok`, `bare_value_cst_node_kind ... ok`.

- [ ] **Step 5: Commit**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/cst/parse.rs
git commit -m "feat(syntax): CST parser — bare-value top-level document body"
```

---

## Task 5: CST lowering — handle `BARE_VALUE` node

**Files:**
- Modify: `crates/mangrove-syntax/src/cst/lower.rs` — `lower()` function

**Interfaces:**
- Consumes: `SyntaxKind::BARE_VALUE` node (from Task 2 + 4); `lower_composite` (already exists)
- Produces: `Document { stmts: vec![], body: <lowered value> }` when `BARE_VALUE` child found

- [ ] **Step 1: Add a `BARE_VALUE` arm in `lower()`**

In `lower.rs`, find the `for child in node.children() { match child.kind() {` loop (~line 27). Find the `_ => {}` catch-all at the end of the match. Add a `BARE_VALUE` arm **before** the catch-all:

```rust
            SyntaxKind::BARE_VALUE => {
                // Bare-value document: lower the single child value.
                // The BARE_VALUE node contains exactly one value child (atom/list/record).
                // We look for the first non-trivia child: a node (composite) or token (scalar).
                let body_value = lower_bare_value_node(&child)?;
                // Return immediately — stmts is empty, body is the value.
                return Ok(Document {
                    uses,
                    typedefs,
                    unitdefs,
                    schema,
                    schema_narrow,
                    params,
                    fns,
                    stmts: vec![],
                    body: body_value,
                });
            }
```

- [ ] **Step 2: Add `lower_bare_value_node` helper**

Add this function after `lower_field` in `lower.rs` (~line 275):

```rust
/// Lower the value inside a BARE_VALUE node.
///
/// A BARE_VALUE node wraps a single value expression at the document level.
/// Its children are: optional leading trivia, the value (a node like LIST/RECORD/REF/UNSET/
/// MATCH_EXPR/CALL, or a scalar token), optional trailing NEWLINE.
fn lower_bare_value_node(node: &SyntaxNode) -> Result<Value, ParseError> {
    use rowan::NodeOrToken;
    for elem in node.children_with_tokens() {
        match elem {
            NodeOrToken::Token(t) if t.kind().is_trivia() => continue,
            // Skip the structural NEWLINE at the end of the bare-value node
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE => continue,
            NodeOrToken::Token(t) => {
                return decode_scalar(&t);
            }
            NodeOrToken::Node(n) => {
                return lower_composite(&n);
            }
        }
    }
    Err(ParseError {
        message: "BARE_VALUE node has no value content".into(),
        line: 0,
        col: 0,
    })
}
```

- [ ] **Step 3: Run the oracle tests to see how many pass now**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test -p mangrove-syntax --lib cst::tests::oracle_bare 2>&1
```

Expected: `oracle_bare_list_document ... ok`, `oracle_bare_int_document ... ok`, `oracle_bare_string_document ... ok`, `oracle_bare_bool_document ... ok`, `oracle_bare_ref_document ... ok`, `oracle_bare_empty_list ... ok`, `oracle_bare_value_with_declarations ... ok`.

- [ ] **Step 4: Commit**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/cst/lower.rs
git commit -m "feat(syntax): CST lowering — BARE_VALUE node lowers to document body value"
```

---

## Task 6: Full green — `just ci` + corpus gate

**Files:**
- Modify if needed: any of the above files to fix issues surfaced by `just ci`

- [ ] **Step 1: Run the full test suite**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && cargo test --workspace --locked 2>&1 | tail -40
```

Expected: all tests pass. Pay attention to:
- `cst_matches_legacy_over_the_example_corpus` — the corpus gate, must still pass
- `oracle_bare_value_hash` — hash oracle for bare-value docs
- All pre-existing oracle/losslessness/declaration tests

- [ ] **Step 2: Fix any failures**

If any test fails, investigate and fix before proceeding. Common issues:
- If `bare_ref_document` fails: check that a bareword at doc level that is NOT `unset`/`match` and has no following `:` is treated as `Value::Ref` in the legacy parser. The `parse_value` method handles this already via `Tok::Bareword(name) => Ok(Value::Ref(name))`.
- If losslessness fails for declarations + bare value: ensure `parse_bare_value_body` consumes the trailing newline ONLY if one exists (the `if p.current() == NEWLINE { p.bump() }` guard).
- If `lower_bare_value_node` returns error: check that `decode_scalar` handles all scalar token kinds (it delegates to the legacy lexer, so it should handle all of them).

- [ ] **Step 3: Run `just ci` (source of truth)**

```bash
cd /Users/moritz/personal/Mangrove/mangrove && just ci 2>&1 | tail -20
```

Expected: all steps pass: `fmt-check`, `lint`, `build`, `test`.

If clippy fails via hook: `rtk proxy cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1` to see real output.

- [ ] **Step 4: Commit the final implementation commit**

```bash
cd /Users/moritz/personal/Mangrove/mangrove
git add crates/mangrove-syntax/src/parser.rs crates/mangrove-syntax/src/cst/kind.rs crates/mangrove-syntax/src/cst/parse.rs crates/mangrove-syntax/src/cst/lower.rs crates/mangrove-syntax/src/cst/tests.rs
git commit -m "feat(syntax): bare-value top-level documents (a document body may be a single value)"
```

---

## Task 7: Write the SDD report

**Files:**
- Create: `/Users/moritz/personal/Mangrove/mangrove/.superpowers/sdd/bareval-report.md`

**Note:** Do NOT `git add` this file.

- [ ] **Step 1: Write the report**

Create `/Users/moritz/personal/Mangrove/mangrove/.superpowers/sdd/bareval-report.md` with content covering:
- Disambiguation rule as implemented (both parsers)
- Changes made to each file (with key function names and line ranges)
- How the legacy `parse_doc` was modified (is_bare_value_start, early-return path)
- How the CST parser was modified (is_bare_value_start_cst, parse_bare_value_body, BARE_VALUE node)
- How lowering was modified (lower_bare_value_node dispatch)
- `{`-leading bodies: unchanged — they fall through to the existing binding logic because `L_BRACE` is not in the bare-value start check
- Oracle additions: list all new oracle/losslessness/hash tests
- RED→GREEN evidence: note which tests were added in Task 1 and confirmed green after Task 5-6
- Corpus gate result: cst_matches_legacy_over_the_example_corpus still passes
- Concerns if any

---

## Self-Review

### Spec coverage

| Requirement | Task |
|---|---|
| `[ 1, 2, 3 ]` parses as doc body == Value::List | Tasks 1, 3, 5 |
| Scalar docs (42, "str", true) parse | Tasks 1, 3, 5 |
| bare-value + declarations + schema works | Tasks 1, 3, 5 |
| Both parsers agree (equivalence oracle) | Tasks 1, 4, 5 |
| CST losslessness on bare-value docs | Tasks 1, 4 |
| `{`-leading bodies unchanged | disambiguation rule in Tasks 3, 4 |
| Corpus gate still green | Task 6 |
| Existing binding-style docs unchanged (regression) | Task 6 |
| Commit to main, no push | Task 6 step 4 |
| Report at .superpowers/sdd/bareval-report.md | Task 7 |
| `schema` binding to bare-value body | Tasks 1 (test), 3, 5 |
| Bare ref (bareword not followed by `:`) | Tasks 1, 3, 4, 5 |
| Empty list `[]` as bare-value doc | Tasks 1, 3, 4, 5 |

### Disambiguation correctness

The rule `is_bare_value_start` / `is_bare_value_start_cst` is symmetric between both parsers:

- `[` → always bare value ✓
- `Int/Decimal/Bool/Bytes/UnitLit/InterpStr` → always bare value ✓
- `Str` → bare value iff next is NOT `:` (quoted key `"k": v` is a binding) ✓
- `unset` / `match` barewords → bare value ✓
- Other barewords → bare value iff next is NOT `:` / `+=` / `{` ✓
- `{` → NOT bare value (falls through to existing logic) ✓
- `...` → spread (handled before the bare-value check) ✓

### Lowering symmetry

Both parsers produce `Document { stmts: vec![], body: <value> }` for bare-value docs. The equivalence oracle (`assert_document_equivalent`) will catch any mismatch.

### Type consistency

- `lower_bare_value_node` returns `Result<Value, ParseError>` — matches the `Value` type used everywhere.
- `SyntaxKind::BARE_VALUE` added before `__LAST` — the `ALL` array is updated so `syntaxkind_all_matches_discriminants` passes.
- `parse_bare_value_body` calls `parse_atom(p, false, 0)` — `stop_at_closer=false` is correct (we're at doc level, not inside a container).

### No placeholders

All steps contain actual code. The key implementation change in `parse_doc` is shown in full (including the unchanged binding path that follows). The `lower_bare_value_node` helper is shown in full.
