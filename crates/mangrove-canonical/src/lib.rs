//! Canonical form pipeline: a `Value` → deterministic CBOR → BLAKE3 content
//! address `b3:<hex>` (spec §7).

use mangrove_core::Value;

/// The canonical CBOR byte representation (spec §7) of a value.
pub fn canonical_cbor(value: &Value) -> Vec<u8> {
    mangrove_cbor::encode(value)
}

/// The content address: `"b3:"` followed by the 64-char lowercase hex of the
/// BLAKE3-256 digest of the canonical CBOR bytes.
pub fn hash(value: &Value) -> String {
    let digest = blake3::hash(&canonical_cbor(value));
    format!("b3:{}", digest.to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_core::Value;

    #[test]
    fn hash_is_b3_prefixed_64_hex() {
        let h = hash(&Value::Bool(true));
        assert!(h.starts_with("b3:"), "{h}");
        assert_eq!(h.len(), 3 + 64);
        assert!(h[3..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_matches_known_vector_for_true() {
        // canonical CBOR of `true` is the single byte 0xf5; this pins its BLAKE3.
        assert_eq!(
            hash(&Value::Bool(true)),
            "b3:518f7263ffb8a410e4874915a1f9a3bd7f11daa2f490abcb00939cdeb002fc81"
        );
    }
}
