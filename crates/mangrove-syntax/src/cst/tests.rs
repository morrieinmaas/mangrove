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

// k8s-templated.mang uses Value::Ref, Value::Match, and params — deferred to Task 10.
// #[test]
// fn oracle_example_k8s_templated() { ... }

use super::kind::SyntaxKind;

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
