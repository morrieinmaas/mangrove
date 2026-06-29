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

/// Parse a YAML document or multi-document stream into a `Value` (schemaless L0 data, D42).
///
/// - A single-document stream returns the document's value directly.
/// - A multi-document stream (documents separated by `---`) returns a
///   `Value::List` where each element is the value of one document.
/// - An empty stream and `null` within any document are still rejected.
///
/// ```
/// let v = mangrove_convert::yaml::import("name: api\nport: 8443\n").unwrap();
/// assert!(matches!(v, mangrove_core::Value::Map(_)));
/// // YAML null is rejected — Mangrove has no null (§2.4).
/// assert!(mangrove_convert::yaml::import("x: null\n").is_err());
/// // Multi-doc stream → Value::List.
/// let multi = mangrove_convert::yaml::import("a: 1\n---\nb: 2\n").unwrap();
/// assert!(matches!(multi, mangrove_core::Value::List(_)));
/// ```
pub fn import(s: &str) -> Result<Value, String> {
    let docs = YamlLoader::load_from_str(s).map_err(|e| format!("YAML parse error: {e}"))?;
    match docs.as_slice() {
        [] => Err("empty YAML document".into()),
        [one] => yaml_to_value(one, "", 0),
        many => {
            let mut out = Vec::with_capacity(many.len());
            for (i, doc) in many.iter().enumerate() {
                out.push(yaml_to_value(doc, &format!("[doc {i}]"), 0)?);
            }
            Ok(Value::List(out))
        }
    }
}

/// Serialize a `Value::List` as a YAML multi-document stream, with each list
/// element emitted as a separate YAML document separated by `\n---\n`.
///
/// If `v` is not a `Value::List`, it is emitted as a single document (identical
/// to [`export`]). The existing [`export`] function is unchanged: a list value
/// still serializes as a single-document YAML sequence by default.
///
/// ```
/// use mangrove_core::Value;
/// use std::collections::BTreeMap;
///
/// let mut m1 = BTreeMap::new();
/// m1.insert("kind".to_string(), Value::Str("PVC".into()));
/// let mut m2 = BTreeMap::new();
/// m2.insert("kind".to_string(), Value::Str("CronJob".into()));
/// let list = Value::List(vec![Value::Map(m1), Value::Map(m2)]);
/// let stream = mangrove_convert::yaml::export_stream(&list).unwrap();
/// assert!(stream.contains("---"));
/// ```
pub fn export_stream(v: &Value) -> Result<String, String> {
    let Value::List(elems) = v else {
        return export(v);
    };
    // YamlEmitter prefixes every document with `---\n` (a document-start marker).
    // Strip that prefix from each piece so that joining with `\n---\n` produces
    // a valid, standard multi-doc stream without doubled markers.
    let mut docs = Vec::with_capacity(elems.len());
    for elem in elems {
        let raw = export(elem)?;
        let body = raw.strip_prefix("---\n").unwrap_or(&raw).to_string();
        docs.push(body);
    }
    Ok(docs.join("\n---\n"))
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

    // ── Multi-document import ──────────────────────────────────────────────────

    #[test]
    fn import_multidoc_two_maps_yields_list() {
        // A k8s-style stream: PVC doc then CronJob doc → Value::List of length 2.
        let yaml = "\
kind: PersistentVolumeClaim
metadata:
  name: pvc
---
kind: CronJob
metadata:
  name: cron
";
        let v = import(yaml).unwrap();
        let Value::List(elems) = v else {
            panic!("expected List, got {v:?}")
        };
        assert_eq!(elems.len(), 2);
        let Value::Map(ref first) = elems[0] else {
            panic!("first elem not Map")
        };
        assert_eq!(
            first.get("kind"),
            Some(&Value::Str("PersistentVolumeClaim".into()))
        );
        let Value::Map(ref second) = elems[1] else {
            panic!("second elem not Map")
        };
        assert_eq!(second.get("kind"), Some(&Value::Str("CronJob".into())));
    }

    #[test]
    fn import_single_doc_not_wrapped_in_list() {
        // Single doc must remain a Map, NOT wrapped in a List.
        let v = import("name: api\nport: 8443\n").unwrap();
        assert!(matches!(v, Value::Map(_)), "expected Map, got {v:?}");
    }

    #[test]
    fn import_empty_still_errors() {
        assert!(import("").is_err());
    }

    #[test]
    fn import_multidoc_with_null_doc_errors() {
        // Null within any document in the stream is still rejected.
        let yaml = "kind: PVC\n---\nx: null\n";
        assert!(import(yaml).is_err());
    }

    // ── export_stream ──────────────────────────────────────────────────────────

    #[test]
    fn export_stream_list_produces_separator() {
        use std::collections::BTreeMap;
        let mut m1 = BTreeMap::new();
        m1.insert("kind".to_string(), Value::Str("PVC".into()));
        let mut m2 = BTreeMap::new();
        m2.insert("kind".to_string(), Value::Str("CronJob".into()));
        let list = Value::List(vec![Value::Map(m1), Value::Map(m2)]);
        let out = export_stream(&list).unwrap();
        // Must contain exactly one `---` separator between the two docs.
        assert!(out.contains("---"), "no separator in: {out}");
        let sep_count = out.matches("---").count();
        assert_eq!(
            sep_count, 1,
            "expected exactly 1 `---`, got {sep_count} in: {out}"
        );
        assert!(out.contains("PVC"));
        assert!(out.contains("CronJob"));
    }

    #[test]
    fn export_stream_non_list_is_single_doc_no_separator() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("kind".to_string(), Value::Str("PVC".into()));
        let v = Value::Map(m.clone());
        let out = export_stream(&v).unwrap();
        // For a non-list, export_stream behaves like export (single doc).
        // YamlEmitter always adds a leading `---` document-start marker; that is
        // fine — what we're checking is that there's no *separator* `---` (i.e.
        // the output equals what plain export produces, not a multi-doc stream).
        assert_eq!(
            out,
            export(&Value::Map(m)).unwrap(),
            "non-list stream differs from export"
        );
        assert!(out.contains("PVC"));
    }

    #[test]
    fn export_stream_list_round_trips() {
        // import(export_stream(list)) == list, and canonical hash is stable.
        use std::collections::BTreeMap;
        let mut m1 = BTreeMap::new();
        m1.insert("kind".to_string(), Value::Str("PVC".into()));
        m1.insert("name".to_string(), Value::Str("my-pvc".into()));
        let mut m2 = BTreeMap::new();
        m2.insert("kind".to_string(), Value::Str("CronJob".into()));
        m2.insert("replicas".to_string(), Value::Int(3.into()));
        let list = Value::List(vec![Value::Map(m1), Value::Map(m2)]);
        let yaml_stream = export_stream(&list).unwrap();
        let roundtripped = import(&yaml_stream).unwrap();
        assert_eq!(list, roundtripped);
        // Canonical hash must be stable.
        assert_eq!(
            mangrove_canonical::hash(&list),
            mangrove_canonical::hash(&roundtripped)
        );
    }

    #[test]
    fn export_unchanged_for_list_produces_single_doc_sequence() {
        // Regression: export (non-stream) of a list still gives a single-doc YAML
        // sequence (unchanged behaviour — multi-doc is opt-in via export_stream).
        let list = Value::List(vec![Value::Int(1.into()), Value::Int(2.into())]);
        let out = export(&list).unwrap();
        // A YAML sequence is rendered as `- 1\n- 2\n`. YamlEmitter prefixes with
        // `---\n` (document-start), but there must be no mid-stream `---` separator.
        // We verify single-doc by ensuring YamlLoader sees exactly one document.
        let docs = YamlLoader::load_from_str(&out).unwrap();
        assert_eq!(docs.len(), 1, "export of a list must be a single YAML doc");
        // Round-trips back to the same List (single-doc sequence → Value::List).
        let back = import(&out).unwrap();
        assert_eq!(list, back);
    }
}
