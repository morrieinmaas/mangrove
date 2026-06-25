//! Deterministic canonical CBOR encoder for `mangrove_core::Value`.
//!
//! Follows RFC 8949 §3 wire format and §4.2 determinism (shortest-form head,
//! definite lengths, no floats) — EXCEPT map keys are ordered by Mangrove spec
//! §7.1 (Unicode code point), not RFC 8949's length-first rule. The §7.1 order
//! is achieved for free by iterating the value's `BTreeMap<String, _>`.

use bigdecimal::BigDecimal;
use mangrove_core::Value;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};

/// Encode a value to its canonical CBOR byte representation.
///
/// ```
/// use mangrove_core::Value;
/// // canonical CBOR of `true` is the single byte 0xf5
/// assert_eq!(mangrove_cbor::encode(&Value::Bool(true)), vec![0xf5]);
/// ```
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Int(n) => encode_int(n, out),
        Value::Decimal(d) => encode_decimal(d, out),
        Value::Str(s) => encode_str(s, out),
        Value::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        Value::Bytes(b) => {
            encode_head(2, b.len() as u64, out);
            out.extend_from_slice(b);
        }
        Value::List(xs) => {
            encode_head(4, xs.len() as u64, out);
            for x in xs {
                encode_into(x, out);
            }
        }
        Value::Map(m) => {
            encode_head(5, m.len() as u64, out);
            for (k, v) in m {
                encode_str(k, out);
                encode_into(v, out);
            }
        }
        // ponytail: a guard, not a code path — unit literals are resolved to a
        // base Int against a schema before hashing (M2b); a schemaless unit
        // literal errors earlier (D14), so this is unreachable in correct flows.
        Value::Unit { .. } => {
            panic!(
                "unresolved unit literal reached the CBOR encoder — resolve against a schema first"
            )
        }
        // ponytail: guard — `unset` is removed during composition (L2); it must
        // never reach the encoder.
        Value::Unset => panic!("`unset` reached the CBOR encoder — compose first"),
        // ponytail: guard — an L3 reference is reduced to its value by the eval
        // stage (M4a); it must never reach the encoder unresolved.
        Value::Ref(name) => {
            panic!("unresolved reference `{name}` reached the CBOR encoder — eval first")
        }
        // ponytail: guard — an interpolated string is reduced to a `Str` by eval.
        Value::Interp(_) => {
            panic!("unresolved interpolation reached the CBOR encoder — eval first")
        }
        // ponytail: guard — a `match` is reduced to the chosen arm by eval.
        Value::Match { .. } => {
            panic!("unresolved `match` reached the CBOR encoder — eval first")
        }
        // ponytail: guard — a function call is reduced to its result by eval.
        Value::Call { .. } => {
            panic!("unresolved function call reached the CBOR encoder — eval first")
        }
        // ponytail: guard — a module call is reduced to its value by eval.
        Value::ModuleCall { .. } => {
            panic!("unresolved module call reached the CBOR encoder — eval first")
        }
    }
}

/// Major type in the top 3 bits; argument in the shortest of the 1/2/3/5/9-byte forms.
fn encode_head(major: u8, arg: u64, out: &mut Vec<u8>) {
    let mt = major << 5;
    if arg < 24 {
        out.push(mt | arg as u8);
    } else if arg <= u8::MAX as u64 {
        out.push(mt | 24);
        out.push(arg as u8);
    } else if arg <= u16::MAX as u64 {
        out.push(mt | 25);
        out.extend_from_slice(&(arg as u16).to_be_bytes());
    } else if arg <= u32::MAX as u64 {
        out.push(mt | 26);
        out.extend_from_slice(&(arg as u32).to_be_bytes());
    } else {
        out.push(mt | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}

fn encode_str(s: &str, out: &mut Vec<u8>) {
    encode_head(3, s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn encode_int(n: &BigInt, out: &mut Vec<u8>) {
    if !n.is_negative() {
        match n.to_u64() {
            Some(u) => encode_head(0, u, out),
            None => encode_bignum_tag(2, n, out),
        }
    } else {
        // CBOR negative integer: value = -1 - arg, so arg = (-n) - 1.
        let arg = -n - BigInt::from(1);
        match arg.to_u64() {
            Some(u) => encode_head(1, u, out),
            None => encode_bignum_tag(3, &arg, out),
        }
    }
}

/// Encode the non-negative `magnitude` of an integer as a CBOR bignum under
/// `tag` (2 = positive, 3 = negative): the tag head, then a byte string of the
/// big-endian minimal-length magnitude.
fn encode_bignum_tag(tag: u64, magnitude: &BigInt, out: &mut Vec<u8>) {
    encode_head(6, tag, out);
    let bytes = magnitude.magnitude().to_bytes_be();
    encode_head(2, bytes.len() as u64, out);
    out.extend_from_slice(&bytes);
}

fn encode_decimal(d: &BigDecimal, out: &mut Vec<u8>) {
    // bigdecimal: value = mantissa * 10^(-scale). CBOR tag 4 = [exponent, mantissa]
    // where value = mantissa * 10^exponent, so exponent = -scale.
    let (mantissa, scale) = d.normalized().as_bigint_and_exponent();
    let exponent = -scale;
    encode_head(6, 4, out); // tag 4 (decimal fraction)
    encode_head(4, 2, out); // array of length 2
    encode_int(&BigInt::from(exponent), out);
    encode_int(&mantissa, out);
}

#[cfg(test)]
mod tests {
    use super::encode;
    use bigdecimal::BigDecimal;
    use mangrove_core::Value;
    use num_bigint::BigInt;
    use std::str::FromStr;

    #[test]
    fn uint_small() {
        assert_eq!(encode(&Value::Int(BigInt::from(0))), vec![0x00]);
    }
    #[test]
    fn uint_23() {
        assert_eq!(encode(&Value::Int(BigInt::from(23))), vec![0x17]);
    }
    #[test]
    fn uint_24() {
        assert_eq!(encode(&Value::Int(BigInt::from(24))), vec![0x18, 0x18]);
    }
    #[test]
    fn uint_1000() {
        assert_eq!(
            encode(&Value::Int(BigInt::from(1000))),
            vec![0x19, 0x03, 0xe8]
        );
    }
    #[test]
    fn negint_m1() {
        assert_eq!(encode(&Value::Int(BigInt::from(-1))), vec![0x20]);
    }
    #[test]
    fn negint_m24() {
        assert_eq!(encode(&Value::Int(BigInt::from(-24))), vec![0x37]);
    }
    #[test]
    fn negint_m25() {
        assert_eq!(encode(&Value::Int(BigInt::from(-25))), vec![0x38, 0x18]);
    }
    #[test]
    fn bignum_pos() {
        // 2^64 = 18446744073709551616 → tag 2 (0xc2), bytestring of 9 bytes (0x49), 01 then 8 zeros.
        let n = BigInt::from_str("18446744073709551616").unwrap();
        assert_eq!(
            encode(&Value::Int(n)),
            vec![0xc2, 0x49, 0x01, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }
    #[test]
    fn boolean() {
        assert_eq!(encode(&Value::Bool(true)), vec![0xf5]);
        assert_eq!(encode(&Value::Bool(false)), vec![0xf4]);
    }
    #[test]
    fn string() {
        assert_eq!(encode(&Value::Str("a".into())), vec![0x61, 0x61]);
    }
    #[test]
    fn bytes() {
        assert_eq!(encode(&Value::Bytes(vec![1, 2, 3])), vec![0x43, 1, 2, 3]);
    }
    #[test]
    fn decimal_1_2() {
        // 1.20 normalizes to 1.2 → tag 4 [exponent=-1, mantissa=12] → c4 82 20 0c
        let d = BigDecimal::from_str("1.20").unwrap();
        assert_eq!(encode(&Value::Decimal(d)), vec![0xc4, 0x82, 0x20, 0x0c]);
    }
    #[test]
    fn decimal_zero() {
        let d = BigDecimal::from_str("0").unwrap();
        assert_eq!(encode(&Value::Decimal(d)), vec![0xc4, 0x82, 0x00, 0x00]);
    }
    #[test]
    fn empty_list_and_map() {
        assert_eq!(encode(&Value::List(vec![])), vec![0x80]);
        assert_eq!(encode(&Value::Map(Default::default())), vec![0xa0]);
    }
    #[test]
    fn list_preserves_order() {
        let v = Value::List(vec![
            Value::Int(BigInt::from(1)),
            Value::Int(BigInt::from(2)),
        ]);
        assert_eq!(encode(&v), vec![0x82, 0x01, 0x02]);
    }
    #[test]
    fn map_keys_sorted_by_codepoint() {
        // Insert out of order; encoding must emit "a" before "b" (§7.1).
        let mut m = std::collections::BTreeMap::new();
        m.insert("b".to_string(), Value::Int(BigInt::from(2)));
        m.insert("a".to_string(), Value::Int(BigInt::from(1)));
        assert_eq!(
            encode(&Value::Map(m)),
            vec![0xa2, 0x61, 0x61, 0x01, 0x61, 0x62, 0x02]
        );
    }
}
