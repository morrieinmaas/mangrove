//! Conformance corpus runner (spec §13).
//!
//! At M0 this only *discovers* vector pairs and checks that every `.mang`
//! input has a matching `.expected` file. M1 adds the parse → canonical form
//! → CBOR → BLAKE3 pipeline and compares the produced `b3:` hash against the
//! `.expected` contents.

use std::fs;
use std::path::{Path, PathBuf};

/// Returns the `(input.mang, input.expected)` vector pairs in `dir`, sorted by
/// input path. Panics if a `.mang` file has no sibling `.expected`.
///
/// ```
/// let dir = std::env::temp_dir().join("mangrove_conformance_doctest");
/// std::fs::create_dir_all(&dir).unwrap();
/// assert!(mangrove_conformance::vector_pairs(&dir).is_empty());
/// ```
pub fn vector_pairs(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("mang") {
            continue;
        }
        let expected = path.with_extension("expected");
        assert!(expected.exists(), "missing .expected for vector {path:?}");
        pairs.push((path, expected));
    }
    pairs.sort();
    pairs
}

/// Parse the `input` document, compute its canonical hash, and assert it equals
/// the trimmed contents of `expected`. Panics with a descriptive message on any
/// mismatch — the test harness surfaces it as a failure.
pub fn run_vector(input: &Path, expected: &Path) {
    let src = fs::read_to_string(input).unwrap_or_else(|e| panic!("read {input:?}: {e}"));
    let want = fs::read_to_string(expected).unwrap_or_else(|e| panic!("read {expected:?}: {e}"));
    let value = mangrove_syntax::parse(&src).unwrap_or_else(|e| panic!("parse {input:?}: {e}"));
    let got = mangrove_canonical::hash(&value);
    assert_eq!(got, want.trim(), "hash mismatch for {input:?}");
}

/// Validate an L1 document and compare its rendered errors to `expected`
/// (spec §13, the `input → expected-errors` vector kind). The rendering is
/// deterministic: errors sorted by `path`, one `path | failed | expected` line
/// each, or the single line `ok` when the document validates.
pub fn run_check_vector(input: &Path, expected: &Path) {
    let src = fs::read_to_string(input).unwrap_or_else(|e| panic!("read {input:?}: {e}"));
    let want = fs::read_to_string(expected).unwrap_or_else(|e| panic!("read {expected:?}: {e}"));
    let got = render_check(&src).unwrap_or_else(|e| panic!("load {input:?}: {e}"));
    assert_eq!(got.trim(), want.trim(), "check mismatch for {input:?}");
}

fn render_check(src: &str) -> Result<String, String> {
    let doc = mangrove_syntax::parse_document(src).map_err(|e| e.to_string())?;
    let env = mangrove_typed::TypeEnv::build(&doc.typedefs, &doc.unitdefs)?;
    let Some(schema_name) = doc.schema else {
        return Ok("ok".to_string());
    };
    let schema_ty = env
        .resolve(&schema_name)
        .ok_or_else(|| format!("unknown schema type: {schema_name}"))?;
    let mut errors = mangrove_typed::validate(&doc.body, schema_ty, &env);
    if errors.is_empty() {
        return Ok("ok".to_string());
    }
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    let lines: Vec<String> = errors
        .iter()
        .map(|e| {
            format!(
                "{} | {} | {}",
                if e.path.is_empty() { "(root)" } else { &e.path },
                e.failed.as_deref().unwrap_or("-"),
                e.expected
            )
        })
        .collect();
    Ok(lines.join("\n"))
}
