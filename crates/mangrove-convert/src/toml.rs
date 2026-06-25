//! TOML ⇄ `Value` (M5c). TOML has no null, so nothing to reject. Caveats:
//! - TOML floats are `f64` in the `toml` crate, so a decimal is preserved only to
//!   f64 precision (its shortest round-trip text) — unlike YAML, which keeps the
//!   source text exactly. Documented limit (D45).
//! - A TOML datetime has no Mangrove scalar, so it imports as the RFC-3339 string
//!   (lossy-but-honest, D44).

use bigdecimal::BigDecimal;
use mangrove_core::Value;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Parse a TOML document into a `Value` (schemaless L0 data, D42).
pub fn import(s: &str) -> Result<Value, String> {
    let table: ::toml::Table = s.parse().map_err(|e| format!("TOML parse error: {e}"))?;
    toml_table_to_value(&table, "")
}

/// Serialize a `Value` (post-eval, no markers) as TOML. The root must be a map.
pub fn export(v: &Value) -> Result<String, String> {
    let Value::Map(_) = v else {
        return Err("a TOML document root must be a map".into());
    };
    let tv = value_to_toml(v)?;
    ::toml::to_string(&tv).map_err(|e| format!("TOML emit error: {e}"))
}

fn toml_table_to_value(t: &::toml::Table, path: &str) -> Result<Value, String> {
    let mut m = BTreeMap::new();
    for (k, v) in t {
        let child = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        m.insert(k.clone(), toml_to_value(v, &child)?);
    }
    Ok(Value::Map(m))
}

fn toml_to_value(v: &::toml::Value, path: &str) -> Result<Value, String> {
    use ::toml::Value as T;
    Ok(match v {
        T::Integer(i) => Value::Int(BigInt::from(*i)),
        // Route through the shortest round-trip text, not the raw f64 bits.
        T::Float(f) => BigDecimal::from_str(&f.to_string())
            .map(Value::Decimal)
            .map_err(|_| format!("{path}: invalid float `{f}`"))?,
        T::String(s) => Value::Str(s.clone()),
        T::Boolean(b) => Value::Bool(*b),
        // No Mangrove datetime scalar → keep the RFC-3339 text (D44).
        T::Datetime(dt) => Value::Str(dt.to_string()),
        T::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for (i, item) in a.iter().enumerate() {
                out.push(toml_to_value(item, &format!("{path}[{i}]"))?);
            }
            Value::List(out)
        }
        T::Table(t) => toml_table_to_value(t, path)?,
    })
}

fn value_to_toml(v: &Value) -> Result<::toml::Value, String> {
    use ::toml::Value as T;
    Ok(match v {
        Value::Int(n) => match n.to_i64() {
            Some(i) => T::Integer(i),
            None => return Err(format!("integer {n} too large for TOML (i64)")),
        },
        Value::Decimal(d) => {
            let f = d
                .to_string()
                .parse::<f64>()
                .map_err(|_| format!("decimal {d} not representable as a TOML float"))?;
            T::Float(f)
        }
        Value::Str(s) => T::String(s.clone()),
        Value::Bool(b) => T::Boolean(*b),
        Value::List(xs) => T::Array(xs.iter().map(value_to_toml).collect::<Result<_, _>>()?),
        Value::Map(m) => {
            let mut t = ::toml::Table::new();
            for (k, val) in m {
                t.insert(k.clone(), value_to_toml(val)?);
            }
            T::Table(t)
        }
        other => return Err(format!("cannot export {other:?} to TOML")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_scalars_tables_arrays() {
        let v = import("name = \"api\"\nport = 8443\nratio = 0.25\n\n[meta]\nflag = true\ntags = [\"a\", \"b\"]\n").unwrap();
        let Value::Map(m) = &v else { panic!() };
        assert_eq!(m.get("name"), Some(&Value::Str("api".into())));
        assert_eq!(m.get("port"), Some(&Value::Int(8443.into())));
        assert_eq!(
            m.get("ratio"),
            Some(&Value::Decimal("0.25".parse().unwrap()))
        );
        let Some(Value::Map(meta)) = m.get("meta") else {
            panic!()
        };
        assert_eq!(meta.get("flag"), Some(&Value::Bool(true)));
        assert_eq!(
            meta.get("tags"),
            Some(&Value::List(vec![
                Value::Str("a".into()),
                Value::Str("b".into())
            ]))
        );
    }

    #[test]
    fn datetime_imports_as_string() {
        let v = import("when = 2026-06-25T10:00:00Z\n").unwrap();
        let Value::Map(m) = &v else { panic!() };
        assert!(matches!(m.get("when"), Some(Value::Str(_))));
    }

    #[test]
    fn round_trip_value_identity() {
        let original = import("a = 1\nb = 0.5\nc = \"hi\"\n\n[d]\ne = [true, 2]\n").unwrap();
        let reparsed = import(&export(&original).unwrap()).unwrap();
        assert_eq!(original, reparsed);
    }
}
