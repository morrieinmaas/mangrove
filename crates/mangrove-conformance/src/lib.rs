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
