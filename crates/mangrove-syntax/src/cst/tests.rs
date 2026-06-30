#[allow(unused_imports)]
use super::{kind::*, lex::*, lower::*, parse::*};

/// The regression net: CST path and the legacy parser must agree on the canonical
/// hash for every input the legacy parser accepts. (Equal values hash equal.)
fn assert_hash_equivalent(src: &str) {
    let legacy = super::super::parse(src); // Result<Value, ParseError>
    let cst = super::lower::lower(&super::parse::parse_cst(src).syntax()).map(|d| d.body);
    match (legacy, cst) {
        (Ok(lv), Ok(cv)) => assert_eq!(
            mangrove_canonical::hash(&lv),
            mangrove_canonical::hash(&cv),
            "hash mismatch for {src:?}"
        ),
        (Err(_), Err(_)) => {} // both reject — fine
        (l, r) => panic!("legacy vs cst disagree for {src:?}: {l:?} / {r:?}"),
    }
}

#[test]
fn oracle_simple_bindings() {
    assert_hash_equivalent("port: 8443\n");
    assert_hash_equivalent("a: true\nb: \"x\"\n");
}

/// Like `assert_hash_equivalent` but compares `Value`s by `PartialEq` instead of
/// hashing. Required for transient markers (`Value::Unit`, `Value::Interp`, etc.)
/// that the CBOR encoder deliberately panics on (they must be resolved before
/// canonicalization). Both legacy and CST must agree on the same Value.
fn assert_value_equivalent(src: &str) {
    let legacy = super::super::parse(src);
    let cst = super::lower::lower(&super::parse::parse_cst(src).syntax()).map(|d| d.body);
    match (legacy, cst) {
        (Ok(lv), Ok(cv)) => assert_eq!(lv, cv, "value mismatch for {src:?}"),
        (Err(_), Err(_)) => {}
        (l, r) => panic!("legacy vs cst disagree for {src:?}: {l:?} / {r:?}"),
    }
}

#[test]
fn oracle_full_scalars() {
    // Fully-resolved scalars: use hash oracle (CBOR-encodable).
    assert_hash_equivalent("x: 0.25\n"); // decimal
    assert_hash_equivalent("t: \"\"\"line one\nline two\"\"\"\n"); // text block, no holes → Str
    assert_hash_equivalent("xs: [ 0.1, 0.2 ]\n");
    // bytes literal: b64"aGVsbG8=" (base64 for "hello"), from cst/lex.rs scans_bytes_literal test
    assert_hash_equivalent("b: b64\"aGVsbG8=\"\n");

    // Transient markers (Unit, Interp): CBOR encoder panics on them by design;
    // compare by PartialEq — both sides must produce the identical Value.
    assert_value_equivalent("x: 512Mi\n"); // unit literal → Value::Unit
    assert_value_equivalent("s: \"hi ${name}\"\n"); // interpolated string → Value::Interp
    assert_value_equivalent("ti: \"\"\"hi ${name}\"\"\"\n"); // text block WITH a hole → Value::Interp
    // composites: contain transient markers, so use value equality
    assert_value_equivalent("m: { mem: 256Mi, cpu: 0.5 }\n");
}

#[test]
fn parses_a_simple_binding_into_a_lossless_tree() {
    let src = "port: 8443\n";
    let parse = super::parse::parse_cst(src);
    let node = parse.syntax();
    // losslessness at the tree level: the tree's text equals the source.
    assert_eq!(node.text().to_string(), src);
    assert_eq!(node.kind(), SyntaxKind::DOCUMENT);
}

#[test]
fn parses_with_leading_trivia_losslessly() {
    for src in [
        "  port: 8443\n",       // leading whitespace
        "# lead\nport: 8443\n", // leading comment
        "\n\nport: 8443\n",     // leading blank lines
    ] {
        let node = super::parse::parse_cst(src).syntax();
        assert_eq!(
            node.text().to_string(),
            src,
            "tree must be lossless for {src:?}"
        );
        assert_eq!(node.kind(), SyntaxKind::DOCUMENT);
    }
}

#[test]
fn oracle_records_and_lists() {
    assert_hash_equivalent("a: { b: 1, c: { d: 2 } }\n");
    assert_hash_equivalent("xs: [ 1, 2, 3 ]\n");
    assert_hash_equivalent("a: { xs: [ 1, 2 ], b: \"x\" }\n");
    // newline-separated fields (as in examples/*.mang)
    assert_hash_equivalent("a: {\n  b: 1\n  c: 2\n}\n");
    // list of records
    assert_hash_equivalent("items: [ { n: 1 }, { n: 2 } ]\n");
    // empty record / empty list
    assert_hash_equivalent("e: {}\n");
    assert_hash_equivalent("e: []\n");
}

#[test]
fn composite_losslessness() {
    let src = "a: { b: [ 1, 2 ] }\n";
    assert_eq!(
        super::parse::parse_cst(src).syntax().text().to_string(),
        src,
        "composite tree must round-trip losslessly"
    );
}

// ---- Task 8: full-Document equivalence oracle ----

fn assert_document_equivalent(src: &str) {
    let legacy = super::super::parse_document(src);
    let cst = super::lower::lower(&super::parse::parse_cst(src).syntax());
    match (legacy, cst) {
        (Ok(l), Ok(c)) => assert_eq!(l, c, "document mismatch for {src:?}"),
        (Err(_), Err(_)) => {}
        (l, r) => panic!("legacy vs cst disagree for {src:?}: {l:?} / {r:?}"),
    }
}

#[test]
fn oracle_declarations() {
    // simple type def + schema + body
    assert_document_equivalent("type T = int\nschema T\nx: 1\n");
    // unit def
    assert_document_equivalent("unit Mem : int { B = 1, Ki = 1024B }\nschema Mem\nx: 1Ki\n");
    // use statement
    assert_document_equivalent("use \"./base.mang\" as base\nschema Base\nx: 1\n");
    // type def with annotation
    assert_document_equivalent(
        "type Port = int & >= 1 & <= 65535 @doc(\"port\")\nschema Port\nx: 1\n",
    );
    // schema with narrow
    assert_document_equivalent(
        "type Base = { a: int }\nschema Base & { b: str }\na: 1\nb: \"x\"\n",
    );
}

#[test]
fn oracle_declaration_losslessness() {
    let srcs = [
        "type T = int\nschema T\nx: 1\n",
        "unit Mem : int { B = 1, Ki = 1024B }\nschema Mem\nx: 1Ki\n",
        "use \"./base.mang\" as base\nschema Base\nx: 1\n",
        "type Port = int & >= 1 & <= 65535\nschema Port\nx: 1\n",
    ];
    for src in srcs {
        let node = super::parse::parse_cst(src).syntax();
        assert_eq!(
            node.text().to_string(),
            src,
            "declaration-bearing input must round-trip losslessly: {src:?}"
        );
    }
}

#[test]
fn oracle_example_k8s_deployment() {
    let p =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/k8s-deployment.mang");
    let src = std::fs::read_to_string(&p).unwrap();
    assert_document_equivalent(&src);
}

#[test]
fn oracle_example_pyproject() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pyproject.mang");
    let src = std::fs::read_to_string(&p).unwrap();
    assert_document_equivalent(&src);
}

// ---- Task 10: templating constructs + corpus gate ----

fn assert_document_equivalent_file(path: &std::path::Path, src: &str) {
    let legacy = super::super::parse_document(src);
    let cst = super::lower::lower(&super::parse::parse_cst(src).syntax());
    match (legacy, cst) {
        (Ok(l), Ok(c)) => assert_eq!(l, c, "document mismatch for file {:?}", path),
        (Err(_), Err(_)) => {}
        (l, r) => panic!("legacy vs cst disagree for file {:?}: {l:?} / {r:?}", path),
    }
}

#[test]
fn oracle_example_k8s_templated() {
    let p =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/k8s-templated.mang");
    let src = std::fs::read_to_string(&p).unwrap();
    assert_document_equivalent(&src);
}

#[test]
fn oracle_templating_constructs() {
    // reference: b references a
    assert_document_equivalent("a: 1\nb: a\n");
    // unset
    assert_document_equivalent("a: unset\n");
    // spread (from parser.rs test at line 1538)
    assert_document_equivalent("use \"./base.mang\" as base\n...base\nport: 9090\ndebug: unset\n");
    // match expression (from k8s-templated.mang)
    assert_document_equivalent(
        "params {\n  env: \"dev\" | \"staging\" | \"prod\" = \"prod\"\n}\nschema Deployment\nreplicas: match env { dev: 1, staging: 2, prod: 6 }\n",
    );
    // ref inside a record
    assert_document_equivalent(
        "params {\n  env: str = \"prod\"\n}\nschema T\nmetadata: {\n  labels: { env: env }\n}\n",
    );
}

#[test]
fn oracle_list_ops() {
    // Append: key += [value, ...]
    assert_document_equivalent(
        "use \"./base.mang\" as base\n...base\nports += [ { containerPort: 9090, name: \"grpc\" } ]\n",
    );
    // ListOp block (patch/append/remove)
    assert_document_equivalent(
        "use \"./base.mang\" as base\n...base\ncontainers { append: { name: \"sidecar\", image: \"envoy:1\" } }\n",
    );
}

#[test]
fn cst_matches_legacy_over_the_example_corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) == Some("mang") {
            let src = std::fs::read_to_string(&p).unwrap();
            assert_document_equivalent_file(&p, &src);
            n += 1;
        }
    }
    assert!(n >= 3, "expected the example corpus, found {n}");
}

// ---- Task 9: full type-grammar equivalence coverage ----

/// Covers every type-grammar form across the full type surface.
/// Syntax for each form was sourced from existing tests in parser.rs, ty.rs,
/// and examples/*.mang — nothing was guessed.
///
/// Most cases pass immediately (the delegation in lower.rs handles them).
/// Any failure points to a segmentation bug (TYPE_DEF node cut short).
#[test]
fn oracle_type_grammar() {
    // 1. Int refinement (§4.3): `int & >= 1 & <= 65535`
    assert_document_equivalent("type P = int & >= 1 & <= 65535\nschema P\nx: 5\n");

    // 2. Decimal range (§4.3): `decimal & >= 0.0 & <= 1.0`
    assert_document_equivalent("type Ratio = decimal & >= 0 & <= 1\nschema Ratio\nx: 0.5\n");

    // 3. Regex (§4.3): from pyproject.mang
    assert_document_equivalent("type S = str & =~ \"^[a-z]+$\"\nschema S\nx: \"abc\"\n");

    // 4. Union of string literals (§4.4): from ty.rs union_of_literals test
    assert_document_equivalent("type Env = \"dev\" | \"prod\"\nschema Env\nx: \"dev\"\n");

    // 5. Union including a primitive: `str | int`
    assert_document_equivalent("type StrOrInt = str | int\nschema StrOrInt\nx: 1\n");

    // 6. Brand (§4.6): from parser.rs brand_type_parses_and_takes_typedef_name test
    assert_document_equivalent("type Satoshis = brand int & >= 0\nschema Satoshis\nx: 1\n");

    // 7. Map type (§4.4): `{ [str]: int }` — from ty.rs map_and_list test
    assert_document_equivalent("type Labels = { [str]: int }\nschema Labels\nx: { key: 1 }\n");

    // 8. List type (§4.4): `[ str ]` — from ty.rs map_and_list test
    assert_document_equivalent("type Tags = [ str ]\nschema Tags\nx: [ \"a\" ]\n");

    // 9. Named type reference: `type A = int; type B = A`
    assert_document_equivalent(
        "type Port = int & >= 1 & <= 65535\ntype Host = str\nschema Port\nx: 8080\n",
    );

    // 10. Annotation on typedef: `@doc("...")` — from parser.rs annotations_parse_on_typedef_and_field
    assert_document_equivalent("type Port = int @doc(\"port number\")\nschema Port\nx: 1\n");

    // 11. @deprecated annotation: from parser.rs annotations_parse_on_typedef_and_field
    assert_document_equivalent(
        "type OldId = str @deprecated(\"use NewId\")\nschema OldId\nx: \"a\"\n",
    );

    // 12. Simple single-line record type
    assert_document_equivalent(
        "type Addr = { host: str, port: int }\nschema Addr\nhost: \"x\"\nport: 8\n",
    );

    // 13. Record type with optional field: from ty.rs record_with_optional test
    assert_document_equivalent(
        "type Cfg = { host: str, port: int, tls?: bool }\nschema Cfg\nhost: \"x\"\nport: 8\n",
    );

    // 14. Record with field default: `| *value` — from ty.rs field_defaults_parse test
    assert_document_equivalent(
        "type Cfg2 = { ns: str | *\"default\", n: int | *0 }\nschema Cfg2\nns: \"x\"\nn: 1\n",
    );

    // 15. Field annotation @key inside record: from k8s-deployment.mang
    //     `ports: [ Port ] @key(name)` — the @key is a field-level annotation inside the record.
    assert_document_equivalent(
        "type Port = { containerPort: int & >= 1 & <= 65535, name: str }\ntype Cont = { ports: [ Port ] @key(name) }\nschema Cont\nports: [ { containerPort: 8080, name: \"http\" } ]\n",
    );

    // 16. Record with `require` predicate (§4.7): from parser.rs require_clause_parses test
    assert_document_equivalent(
        "type Bounds = { a: int, b: int, require: a <= b @message(\"a must be <= b\") }\nschema Bounds\na: 1\nb: 2\n",
    );

    // 17. Multi-line record type: the form most likely to expose a segmentation gap.
    //     (From k8s-deployment.mang Container type — multi-line with field annotations.)
    assert_document_equivalent(
        "type Port = { containerPort: int & >= 1 & <= 65535, name: str }\ntype Container = {\n  name: str,\n  image: str,\n  ports: [ Port ],\n}\nschema Container\nname: \"api\"\nimage: \"reg/img:1\"\nports: [ { containerPort: 8443, name: \"https\" } ]\n",
    );

    // 18. Annotation on the typedef itself AFTER a record closing brace:
    //     `type R = { a: int } @doc("x")` — this is the known segmentation-gap case.
    assert_document_equivalent("type R = { a: int } @doc(\"annotated record\")\nschema R\na: 1\n");
}

/// Losslessness assertion for a multi-line type definition: the CST tree text
/// must round-trip byte-for-byte.
#[test]
fn multiline_type_def_losslessness() {
    let src = "type Container = {\n  name: str,\n  image: str,\n  ports: [ int ],\n}\nschema Container\nname: \"api\"\nimage: \"reg/img:1\"\nports: [ 8443 ]\n";
    let node = super::parse::parse_cst(src).syntax();
    assert_eq!(
        node.text().to_string(),
        src,
        "multi-line type def must round-trip losslessly"
    );
}

// ---- Task 11: error recovery ----

#[test]
fn recovers_from_a_bad_binding_and_keeps_parsing() {
    let src = "a: @@@\nb: 2\n"; // `@@@` is garbage in value position
    let parse = super::parse::parse_cst(src);
    assert_eq!(parse.syntax().text().to_string(), src); // still lossless
    assert!(!parse.errors.is_empty()); // error recorded
    // `b: 2` still parsed as a BINDING (resynced after the newline)
    let bindings = parse
        .syntax()
        .descendants()
        .filter(|n| n.kind() == super::kind::SyntaxKind::BINDING)
        .count();
    assert_eq!(bindings, 2);
}

#[test]
fn parse_cst_never_panics_on_fuzzed_garbage() {
    for src in [
        "",
        "{{{{",
        "type =",
        ": : :",
        "\"unterminated",
        "a: [1, 2",
        "}}}}",
        "@@@\n@@@",
        // C1 inputs: foreign closer inside container — previously caused infinite loop.
        // RED evidence: before the fix, these hang; run under timeout to confirm.
        "x: [ } ]\n",
        "x: { ] }\n",
        "[ }",
        "{ ]",
    ] {
        let p = super::parse::parse_cst(src);
        assert_eq!(p.syntax().text().to_string(), src); // lossless even when broken
    }
}

// ---- C2: non-ASCII tokens must not panic ----

/// C2: parse_cst must NOT panic and must be lossless for inputs containing non-ASCII
/// characters in token position (multi-byte UTF-8 lead bytes hit the ERROR fallback in
/// scan_significant; the fix advances by the full char width instead of 1 byte).
#[test]
fn parse_cst_non_ascii_tokens_no_panic_and_lossless() {
    for src in [
        "é: 1\n",    // 2-byte UTF-8 lead byte (U+00E9) as key
        "café: 1\n", // multi-char key with non-ASCII
        "💀: 1\n",   // 4-byte astral codepoint (U+1F480)
        "a: ☃\n",    // non-ASCII in value position (U+2603)
    ] {
        let p = super::parse::parse_cst(src);
        assert_eq!(
            p.syntax().text().to_string(),
            src,
            "parse_cst must be lossless for non-ASCII input {src:?}"
        );
    }
}

// ---- I1: CST SyntaxKind must match legacy lexer Tok kind for significant tokens ----

/// I1: For each significant (non-trivia) CST token, its SyntaxKind must agree with
/// the kind that the legacy lexer assigns to that same text when re-lexed in isolation.
/// This catches wrong-SyntaxKind bugs that the document-equivalence oracle misses
/// (because lower.rs delegates leaf decoding back to the legacy lexer).
#[test]
fn cst_token_kinds_match_legacy_lexer_over_corpus() {
    use super::super::lexer::{Tok, lex};
    use super::kind::SyntaxKind;

    fn tok_to_expected_syntax_kind(tok: &Tok) -> Option<SyntaxKind> {
        match tok {
            Tok::LBrace => Some(SyntaxKind::L_BRACE),
            Tok::RBrace => Some(SyntaxKind::R_BRACE),
            Tok::LBracket => Some(SyntaxKind::L_BRACKET),
            Tok::RBracket => Some(SyntaxKind::R_BRACKET),
            Tok::LParen => Some(SyntaxKind::L_PAREN),
            Tok::RParen => Some(SyntaxKind::R_PAREN),
            Tok::Colon => Some(SyntaxKind::COLON),
            Tok::Comma => Some(SyntaxKind::COMMA),
            Tok::Newline => Some(SyntaxKind::NEWLINE),
            Tok::Amp => Some(SyntaxKind::AMP),
            Tok::Pipe => Some(SyntaxKind::PIPE),
            Tok::Eq => Some(SyntaxKind::EQ),
            Tok::Match => Some(SyntaxKind::MATCH),
            Tok::Question => Some(SyntaxKind::QUESTION),
            Tok::Ge => Some(SyntaxKind::GE),
            Tok::Le => Some(SyntaxKind::LE),
            Tok::Gt => Some(SyntaxKind::GT),
            Tok::Lt => Some(SyntaxKind::LT),
            Tok::Star => Some(SyntaxKind::STAR),
            Tok::At => Some(SyntaxKind::AT),
            Tok::Dot => Some(SyntaxKind::DOT),
            Tok::DotDotDot => Some(SyntaxKind::DOT_DOT_DOT),
            Tok::EqEq => Some(SyntaxKind::EQ_EQ),
            Tok::Ne => Some(SyntaxKind::NE),
            Tok::Bang => Some(SyntaxKind::BANG),
            Tok::AmpAmp => Some(SyntaxKind::AMP_AMP),
            Tok::PipePipe => Some(SyntaxKind::PIPE_PIPE),
            Tok::PlusEq => Some(SyntaxKind::PLUS_EQ),
            Tok::Int(_) => Some(SyntaxKind::INT),
            Tok::Decimal(_) => Some(SyntaxKind::DECIMAL),
            Tok::UnitLit(_, _) => Some(SyntaxKind::UNIT_LIT),
            Tok::Str(_) => Some(SyntaxKind::STR),
            Tok::InterpStr(_) => Some(SyntaxKind::STR), // CST emits STR for interp strings too
            Tok::Bool(_) => Some(SyntaxKind::BOOL),
            Tok::Bytes(_) => Some(SyntaxKind::BYTES),
            Tok::Bareword(_) => Some(SyntaxKind::BAREWORD),
            Tok::Doc(_) => Some(SyntaxKind::DOC),
            Tok::Directive(_) => Some(SyntaxKind::DIRECTIVE),
            Tok::Eof => None, // skip
        }
    }

    fn check_src(src: &str) {
        // Walk CST significant tokens and re-lex each token text in isolation.
        let parse = super::parse::parse_cst(src);
        let root = parse.syntax();
        for tok in root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            let sk = tok.kind();
            if sk.is_trivia() || sk == SyntaxKind::EOF || sk == SyntaxKind::ERROR {
                continue;
            }
            // Skip node-level kinds and structural kinds that the legacy lexer
            // doesn't produce as standalone tokens (NEWLINE is produced but
            // skip it as it appears in both CST and legacy consistently).
            let text = tok.text();
            // Re-lex the token text in isolation via the legacy lexer.
            // The legacy lexer skips whitespace, so we need the raw text.
            // Append a space to prevent the legacy lexer from merging tokens.
            let probe = format!("{text} ");
            let Ok(legacy_toks) = lex(&probe) else {
                // Legacy lexer errors on unknown bytes — that's fine, CST emits ERROR for those.
                // If CST emitted a non-ERROR kind for something the legacy lexer rejects, skip.
                continue;
            };
            // legacy_toks: [significant_token, Eof] (whitespace skipped)
            // or [Newline, Eof] for newlines
            let first_legacy = legacy_toks.first().map(|t| &t.tok);
            let Some(first_legacy) = first_legacy else {
                continue;
            };
            if matches!(first_legacy, Tok::Eof) {
                continue;
            }

            if let Some(expected_sk) = tok_to_expected_syntax_kind(first_legacy) {
                assert_eq!(
                    sk, expected_sk,
                    "CST token kind mismatch for text {text:?}: CST={sk:?}, legacy expects {expected_sk:?}"
                );
            }
        }
    }

    // Inline fixtures covering each significant token kind
    for src in [
        "port: 8443\n",
        "x: 0.25\n",
        "x: 512Mi\n",
        "x: \"hello\"\n",
        "x: true\n",
        "x: false\n",
        "x: b64\"aGVsbG8=\"\n",
        "x: schema\n",
        "x: [ 1, 2 ]\n",
        "x: { a: 1 }\n",
        "type T = int & >= 1\n",
        "x: y\n",
        "x: unset\n",
    ] {
        check_src(src);
    }

    // Corpus: all example .mang files
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) == Some("mang") {
            let src = std::fs::read_to_string(&p).unwrap();
            check_src(&src);
        }
    }
}

#[test]
fn syntaxkind_all_matches_discriminants() {
    // ALL must list every variant in discriminant order, and have length __LAST.
    assert_eq!(SyntaxKind::ALL.len(), SyntaxKind::__LAST as usize);
    for (i, k) in SyntaxKind::ALL.iter().enumerate() {
        assert_eq!(
            *k as usize, i,
            "ALL[{i}] = {k:?} is out of discriminant order"
        );
        assert_eq!(SyntaxKind::from_u16(i as u16), Some(*k));
    }
}

#[test]
fn tree_has_expected_kinds_at_paths() {
    use super::kind::SyntaxKind::*;
    // helper: collect (kind, text) of all non-trivia tokens under the first node of `kind`.
    fn first_node(root: &SyntaxNode, k: SyntaxKind) -> SyntaxNode {
        root.descendants()
            .find(|n| n.kind() == k)
            .expect("node kind present")
    }
    // a binding with a record value
    let p = parse_cst("a: { b: 1, c: [ 2, 3 ] }\n");
    let root = p.syntax();
    assert_eq!(root.kind(), DOCUMENT);
    let binding = first_node(&root, BINDING);
    // the record value is a RECORD node, its fields are FIELD nodes
    let record = first_node(&binding, RECORD);
    let fields: Vec<_> = record.children().filter(|n| n.kind() == FIELD).collect();
    assert_eq!(fields.len(), 2, "record has two FIELD children");
    // the list value is a LIST node with INT element tokens
    let list = first_node(&record, LIST);
    let ints: Vec<_> = list
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == INT)
        .collect();
    assert_eq!(ints.len(), 2, "list has two INT tokens");
    // a type def is a TYPE_DEF node; a templating ref value is a REF node; unset is UNSET
    let p2 = parse_cst("type T = int\nschema T\nx: y\nz: unset\n");
    let r2 = p2.syntax();
    assert_eq!(first_node(&r2, TYPE_DEF).kind(), TYPE_DEF);
    assert_eq!(first_node(&r2, REF).kind(), REF);
    assert_eq!(first_node(&r2, UNSET).kind(), UNSET);
}

// ---- Depth limit: no stack overflow on deeply nested input ----

/// D1: parse_cst on a deeply-nested value MUST return (not abort) AND be lossless
/// AND record at least one error. Before the fix this ABORTS via SIGABRT (stack
/// overflow — uncatchable by catch_unwind). To confirm RED: build the binary and
/// run `timeout 15 <bin> fmt <file>`; exit 134 = SIGABRT.
#[test]
fn deeply_nested_value_does_not_overflow() {
    // 100_000 open brackets (far beyond the 128-deep cap)
    let src_list = format!("x: {}", "[".repeat(100_000));
    let parse_list = super::parse::parse_cst(&src_list);
    assert_eq!(
        parse_list.syntax().text().to_string(),
        src_list,
        "lossless on deep list nesting"
    );
    assert!(
        !parse_list.errors.is_empty(),
        "should record a depth error for deeply nested list"
    );

    // 100_000 open records (each `{a: ` — depth exceeds cap)
    let src_rec = "a: ".to_string() + &"{a: ".repeat(100_000);
    let parse_rec = super::parse::parse_cst(&src_rec);
    assert_eq!(
        parse_rec.syntax().text().to_string(),
        src_rec,
        "lossless on deep record nesting"
    );
    assert!(
        !parse_rec.errors.is_empty(),
        "should record a depth error for deeply nested record"
    );
}

// ---- list spread oracle ----

#[test]
fn oracle_list_spread_both_frontends_agree() {
    // The value-equivalent oracle is required here because ListSpread is a
    // transient marker (not CBOR-encodable); both parsers must produce the same Value.
    assert_value_equivalent("xs: [ 1, 2 ]\nys: [ 0, ...xs, 3 ]\n");
    assert_value_equivalent("a: [ ...[] ]\n");
    assert_value_equivalent("v: [ ...[1, ...[2, 3]] ]\n");
    assert_value_equivalent(
        "start: [ ...a, 1 ]\nmid: [ 1, ...a, 2 ]\nend: [ 1, ...a ]\na: [ 9 ]\n",
    );
}

#[test]
fn list_spread_cst_losslessness() {
    for src in [
        "ys: [ 0, ...xs, 3 ]\n",
        "a: [ ...[] ]\n",
        "b: [ ...[1, 2], 3 ]\n",
    ] {
        let node = super::parse::parse_cst(src).syntax();
        assert_eq!(
            node.text().to_string(),
            src,
            "list spread must round-trip losslessly: {src:?}"
        );
    }
}

#[test]
fn list_spread_cst_node_kind() {
    // The CST must emit a LIST_SPREAD node inside a LIST for `...expr`
    let p = super::parse::parse_cst("xs: [ 0, ...ys, 3 ]\n");
    let root = p.syntax();
    let list_spread = root
        .descendants()
        .find(|n| n.kind() == super::kind::SyntaxKind::LIST_SPREAD);
    assert!(
        list_spread.is_some(),
        "expected a LIST_SPREAD child inside a LIST for `...ys`"
    );
}

/// D2: a reasonably nested valid input (10-deep) still parses correctly and
/// round-trips through lower with NO errors — the depth cap does not affect
/// real-world inputs.
#[test]
fn moderately_nested_input_parses_normally() {
    // 10-deep list: well below MAX_DEPTH
    let depth = 10usize;
    let src = format!("x: {}{}", "[".repeat(depth), "]".repeat(depth));
    let parse = super::parse::parse_cst(&src);
    assert_eq!(
        parse.syntax().text().to_string(),
        src,
        "lossless on 10-deep list"
    );
    assert!(
        parse.errors.is_empty(),
        "no errors on a valid 10-deep list: {:?}",
        parse.errors
    );

    // Also verify round-trip via oracle
    assert_hash_equivalent(&src);
}

fn relex_roundtrips(src: &str) {
    let toks = super::lex::lex_lossless(src);
    let joined: String = toks.iter().map(|t| &src[t.start..t.end]).collect();
    assert_eq!(
        joined, src,
        "lossless lexer must reproduce the source byte-for-byte"
    );
    assert_eq!(toks.last().unwrap().kind, SyntaxKind::EOF);
}

#[test]
fn lexer_is_lossless_on_examples() {
    for src in [
        "host: \"api.eu\"\nport: 8443\n",
        "# a comment\ntype T = int  # trailing\nschema T\nx: 1\n",
        "a: { b: [ 1, 2 ], c: 512Mi }\n",
        "s: \"\"\"text ${v}\nblock\"\"\"\n",
        "",
        "   \n\t# only trivia\n",
    ] {
        relex_roundtrips(src);
    }
}

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
    assert_document_equivalent("type Port = int & >= 1 & <= 65535\nschema Port\n[ 8443, 9090 ]\n");
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

/// M4: bare `unset` and `match` as document bodies — the oracle must agree
/// (both produce Value::Unset / Value::Match respectively) without panicking.
/// These are `assert_value_equivalent` rather than `assert_hash_equivalent`
/// because CBOR panics on Unset/Match by design; we only need both parsers to
/// agree on the *value* they produce.
#[test]
fn oracle_bare_unset_document() {
    // Both parsers must agree that a bare `unset` document produces Value::Unset.
    assert_value_equivalent("unset\n");
}

#[test]
fn oracle_bare_match_document() {
    // Both parsers must agree on a bare `match` expression document.
    assert_value_equivalent(
        "params {\n  env: \"dev\" | \"prod\" = \"dev\"\n}\nmatch env { dev: 1, prod: 2 }\n",
    );
}

#[test]
fn bare_value_cst_node_kind() {
    // The CST must emit a BARE_VALUE node as a direct child of DOCUMENT
    let p = super::parse::parse_cst("[ 1, 2, 3 ]\n");
    let root = p.syntax();
    let bare_val = root.children().find(|n| n.kind() == SyntaxKind::BARE_VALUE);
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

// ---- conditional list elements (`item if cond`) ----

#[test]
fn oracle_cond_elem_both_frontends_agree() {
    // Both parsers must produce the same Value (ListSpread(Match{...})) for `item if cond`.
    // Uses assert_value_equivalent because ListSpread is a transient marker.
    assert_value_equivalent("on: true\nout: [ \"a\", \"b\" if on ]\n");
    assert_value_equivalent("on: false\nout: [ \"a\", \"b\" if on ]\n");
    assert_value_equivalent("xs: [ 1, 2 if true, 3 ]\n");
    assert_value_equivalent("xs: [ 1 if false ]\n");
    // Compound items in conditional elements — the CST must agree with legacy.
    assert_value_equivalent("on: true\nout: [ { k: 1 } if on ]\n");
    assert_value_equivalent("on: false\nout: [ { k: 1 } if on ]\n");
    assert_value_equivalent("on: true\nout: [ [ 10, 20 ] if on ]\n");
    assert_value_equivalent("on: false\nout: [ [ 10, 20 ] if on ]\n");
    // Spread of compound (inner is a list literal containing a record)
    assert_value_equivalent("out: [ ...[ { a: 1 } ] ]\n");
    // Mixed: scalar + compound conditional elements together
    assert_value_equivalent(
        "on: true\nout: [ { kind: \"A\" }, { kind: \"B\" } if on, \"c\" if on ]\n",
    );
}

#[test]
fn cst_no_cond_elem_across_newline() {
    // `item\nif cond` must NOT create a COND_ELEM in the CST: the `if` must be
    // on the same logical line as the item. `p.current()` skips only
    // WHITESPACE/COMMENT (not NEWLINE), so a NEWLINE between item and `if`
    // means `p.current()` returns NEWLINE, blocking the conditional-element path.
    let src = "xs: [ \"x\"\n if on ]\n";
    let p = super::parse::parse_cst(src);
    let cond_elem = p
        .syntax()
        .descendants()
        .find(|n| n.kind() == super::kind::SyntaxKind::COND_ELEM);
    assert!(
        cond_elem.is_none(),
        "cross-newline `if` must not produce a COND_ELEM node"
    );
}

#[test]
fn cond_elem_cst_losslessness() {
    for src in [
        "out: [ \"a\", \"b\" if on ]\n",
        "xs: [ 1, 2 if true, 3 ]\n",
        "xs: [ x if flag ]\n",
    ] {
        let node = super::parse::parse_cst(src).syntax();
        assert_eq!(
            node.text().to_string(),
            src,
            "conditional element must round-trip losslessly: {src:?}"
        );
    }
}

#[test]
fn cond_elem_cst_node_kind() {
    // The CST must emit a COND_ELEM node inside a LIST for `item if cond`
    let p = super::parse::parse_cst("xs: [ 1, 2 if flag ]\n");
    let root = p.syntax();
    let cond_elem = root
        .descendants()
        .find(|n| n.kind() == super::kind::SyntaxKind::COND_ELEM);
    assert!(
        cond_elem.is_some(),
        "expected a COND_ELEM child inside a LIST for `2 if flag`"
    );
}

// ---- CST/legacy parity: cross-newline and missing-separator list input ----

/// Regression: the CST previously accepted `[ 1 2 ]` (elements with no separator)
/// while the legacy parser rejected it. Both frontends must now agree (both error).
#[test]
fn cst_missing_separator_in_list_matches_legacy() {
    // No separator between two adjacent elements — legacy errors, CST must also error.
    assert_document_equivalent("xs: [ 1 2 ]\n");
    assert_document_equivalent("xs: [ \"a\" \"b\" ]\n");
}

/// Regression: in `[ "x"\n if on ]` the legacy parser errors ("expected ',' or
/// newline in list" — because after `skip_seps` consumes the newline, `if` and `on`
/// become two adjacent bare elements with no separator).  The CST used to silently
/// produce `List([Str("x"), Ref("if"), Ref("on")])`.  Both frontends now agree (both
/// error).
#[test]
fn cst_cross_newline_if_without_separator_matches_legacy() {
    // The `if` on the next line is a bareword element; `on` follows with no separator.
    assert_document_equivalent("xs: [ \"x\"\n if on ]\n");
    assert_document_equivalent("on: true\nout: [ \"a\"\nif on ]\n");
}
