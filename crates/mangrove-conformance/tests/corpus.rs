use mangrove_conformance::vector_pairs;
use std::path::Path;

/// Absolute path to the L0 vector directory at the workspace root.
const L0_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/l0");

#[test]
fn every_l0_input_has_an_expected_file() {
    let pairs = vector_pairs(Path::new(L0_CORPUS));
    assert!(
        !pairs.is_empty(),
        "no conformance vectors found under {L0_CORPUS}"
    );
}
