//! The L0 value model. Construction implies canonical form: map keys are
//! sorted (BTreeMap), and numbers are expected normalized by the producer.

use bigdecimal::BigDecimal;
use num_bigint::BigInt;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(BigInt),
    Decimal(BigDecimal),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    /// An unresolved unit literal (`512Mi`), carrying its mantissa and suffix.
    /// Resolved to a base `Int` against a unit type before canonicalization
    /// (M2b); it must never reach the CBOR encoder unresolved.
    Unit {
        mantissa: BigDecimal,
        suffix: String,
    },
}

impl Value {
    /// Test/debug helper: keys of a `Map` in canonical (sorted) order; empty for non-maps.
    pub fn map_keys(&self) -> Vec<String> {
        match self {
            Value::Map(m) => m.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_value_constructs() {
        let u = Value::Unit {
            mantissa: 512.into(),
            suffix: "Mi".into(),
        };
        assert!(matches!(u, Value::Unit { .. }));
    }

    #[test]
    fn map_iterates_in_codepoint_key_order() {
        let mut m = BTreeMap::new();
        m.insert("b".to_string(), Value::Int(BigInt::from(2)));
        m.insert("a".to_string(), Value::Int(BigInt::from(1)));
        assert_eq!(
            Value::Map(m).map_keys(),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
