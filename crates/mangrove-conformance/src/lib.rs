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
