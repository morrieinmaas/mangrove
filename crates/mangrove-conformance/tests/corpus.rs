use mangrove_conformance::vector_pairs;
use std::path::Path;

/// Absolute paths to the vector directories at the workspace root.
const L0_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/l0");
const L1_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/l1");

#[test]
fn every_l0_input_has_an_expected_file() {
    let pairs = vector_pairs(Path::new(L0_CORPUS));
    assert!(
        !pairs.is_empty(),
        "no conformance vectors found under {L0_CORPUS}"
    );
}

#[test]
fn all_l0_vectors_hash_to_expected() {
    for (input, expected) in vector_pairs(Path::new(L0_CORPUS)) {
        mangrove_conformance::run_vector(&input, &expected);
    }
}

#[test]
fn all_l1_vectors_match_expected() {
    let pairs = vector_pairs(Path::new(L1_CORPUS));
    assert!(!pairs.is_empty(), "no L1 vectors found under {L1_CORPUS}");
    for (input, expected) in pairs {
        mangrove_conformance::run_check_vector(&input, &expected);
    }
}
