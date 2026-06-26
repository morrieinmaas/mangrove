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
    ] {
        let p = super::parse::parse_cst(src);
        assert_eq!(p.syntax().text().to_string(), src); // lossless even when broken
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
