//! YAML ⇄ `Value` (M5b). Decimals keep their source text (no `f64`, D45);
//! integers are arbitrary precision; YAML `null` and non-string keys are errors.

use bigdecimal::BigDecimal;
use mangrove_core::Value;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::collections::BTreeMap;
use std::str::FromStr;
use yaml_rust2::yaml::{Hash, Yaml};
use yaml_rust2::{YamlEmitter, YamlLoader};

/// Bound on nesting depth (matches the parser) — guards both directions against a
/// stack overflow on adversarial input, rather than relying on the YAML library's.
const MAX_DEPTH: usize = 128;

/// Parse a single YAML document into a `Value` (schemaless L0 data, D42).
///
/// ```
/// let v = mangrove_convert::yaml::import("name: api\nport: 8443\n").unwrap();
/// assert!(matches!(v, mangrove_core::Value::Map(_)));
/// // YAML null is rejected — Mangrove has no null (§2.4).
/// assert!(mangrove_convert::yaml::import("x: null\n").is_err());
/// ```
pub fn import(s: &str) -> Result<Value, String> {
    let docs = YamlLoader::load_from_str(s).map_err(|e| format!("YAML parse error: {e}"))?;
    match docs.as_slice() {
        [] => Err("empty YAML document".into()),
        [one] => yaml_to_value(one, "", 0),
        _ => Err(format!(
            "expected a single YAML document, found {}",
            docs.len()
        )),
    }
}

/// Serialize a `Value` (post-eval, no markers) as YAML.
pub fn export(v: &Value) -> Result<String, String> {
    let y = value_to_yaml(v, 0)?;
    let mut out = String::new();
    YamlEmitter::new(&mut out)
        .dump(&y)
        .map_err(|e| format!("YAML emit error: {e}"))?;
    Ok(out)
}

fn yaml_to_value(y: &Yaml, path: &str, depth: usize) -> Result<Value, String> {
    if depth >= MAX_DEPTH {
        return Err(format!("{path}: nesting too deep"));
    }
    match y {
        Yaml::Integer(i) => Ok(Value::Int(BigInt::from(*i))),
        // yaml-rust2 surfaces an integer beyond i64 as a `Real`; keep integer
        // kind (D45 arbitrary-precision int) when the source text is integral.
        Yaml::Real(s) => {
            if !s.contains(['.', 'e', 'E'])
                && let Ok(n) = BigInt::from_str(s)
            {
                return Ok(Value::Int(n));
            }
            BigDecimal::from_str(s)
                .map(Value::Decimal)
                .map_err(|_| format!("{path}: invalid number `{s}`"))
        }
        Yaml::String(s) => Ok(Value::Str(s.clone())),
        Yaml::Boolean(b) => Ok(Value::Bool(*b)),
        Yaml::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for (i, item) in a.iter().enumerate() {
                out.push(yaml_to_value(item, &format!("{path}[{i}]"), depth + 1)?);
            }
            Ok(Value::List(out))
        }
        Yaml::Hash(h) => {
            let mut m = BTreeMap::new();
            for (k, v) in h {
                let Yaml::String(key) = k else {
                    return Err(format!("{path}: only string keys are supported"));
                };
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                m.insert(key.clone(), yaml_to_value(v, &child, depth + 1)?);
            }
            Ok(Value::Map(m))
        }
        Yaml::Null => Err(format!(
            "{path}: null is not allowed (Mangrove has no null, §2.4)"
        )),
        Yaml::Alias(_) => Err(format!("{path}: YAML aliases are not supported")),
        Yaml::BadValue => Err(format!("{path}: malformed YAML value")),
    }
}

fn value_to_yaml(v: &Value, depth: usize) -> Result<Yaml, String> {
    if depth >= MAX_DEPTH {
        return Err("nesting too deep".into());
    }
    Ok(match v {
        // An int beyond i64 is emitted as its decimal text via `Real`, so it
        // re-imports as a number, not a string.
        Value::Int(n) => match n.to_i64() {
            Some(i) => Yaml::Integer(i),
            None => Yaml::Real(n.to_string()),
        },
        Value::Decimal(d) => Yaml::Real(d.normalized().to_string()),
        Value::Str(s) => Yaml::String(s.clone()),
        Value::Bool(b) => Yaml::Boolean(*b),
        Value::List(xs) => Yaml::Array(
            xs.iter()
                .map(|x| value_to_yaml(x, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Map(m) => {
            let mut h = Hash::new();
            for (k, val) in m {
                h.insert(Yaml::String(k.clone()), value_to_yaml(val, depth + 1)?);
            }
            Yaml::Hash(h)
        }
        other => return Err(format!("cannot export {other:?} to YAML")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_scalars_and_nesting() {
        let v = import("name: api\nport: 8443\nratio: 0.25\nflag: true\ntags:\n  - a\n  - b\n")
            .unwrap();
        let Value::Map(m) = &v else { panic!() };
        assert_eq!(m.get("name"), Some(&Value::Str("api".into())));
        assert_eq!(m.get("port"), Some(&Value::Int(8443.into())));
        assert_eq!(
            m.get("ratio"),
            Some(&Value::Decimal("0.25".parse().unwrap()))
        );
        assert_eq!(m.get("flag"), Some(&Value::Bool(true)));
        assert_eq!(
            m.get("tags"),
            Some(&Value::List(vec![
                Value::Str("a".into()),
                Value::Str("b".into())
            ]))
        );
    }

    #[test]
    fn null_is_rejected() {
        assert!(import("a: null\n").is_err());
        assert!(import("a: ~\n").is_err());
    }

    #[test]
    fn non_string_key_is_rejected() {
        assert!(import("1: a\n").is_err());
    }

    #[test]
    fn decimal_keeps_precision_no_f64() {
        // A value f64 cannot hold exactly must survive verbatim.
        let v = import("x: 0.123456789012345678\n").unwrap();
        let Value::Map(m) = &v else { panic!() };
        assert_eq!(
            m.get("x"),
            Some(&Value::Decimal("0.123456789012345678".parse().unwrap()))
        );
    }

    #[test]
    fn round_trip_value_identity() {
        // yaml → Value → yaml → Value preserves the value (D43).
        let original = import("a: 1\nb: 0.5\nc:\n  d: hi\n  e:\n    - true\n    - 2\n").unwrap();
        let reparsed = import(&export(&original).unwrap()).unwrap();
        assert_eq!(original, reparsed);
    }

    #[test]
    fn large_integer_keeps_int_kind() {
        // Beyond i64, yaml-rust2 yields a Real; an integral one stays an Int (D45).
        let v = import("big: 99999999999999999999999999\n").unwrap();
        let Value::Map(m) = &v else { panic!() };
        assert_eq!(
            m.get("big"),
            Some(&Value::Int("99999999999999999999999999".parse().unwrap()))
        );
    }

    #[test]
    fn deeply_nested_errors_not_overflows() {
        let deep = format!("{}1{}", "[".repeat(10_000), "]".repeat(10_000));
        let src = format!("a: {deep}\n");
        // Either the YAML lib or our depth guard rejects it — never a SIGABRT.
        assert!(import(&src).is_err());
    }
}
