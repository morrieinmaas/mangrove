//! The resolution pass: turn a parsed value into its canonical *resolved* form
//! against a schema (§7, Decision D12) — unit literals become their base
//! integer. Everything else is copied through. A schemaless document is never
//! passed here (it resolves to itself). The output is what gets hashed.

use crate::env::TypeEnv;
use crate::validate::{child, render};
use mangrove_core::Value;
use mangrove_core::error::ValidationError;
use mangrove_syntax::Type;
use std::collections::BTreeMap;

/// Resolve `value` against `ty`, producing the canonical value to hash.
/// Errors on an unresolvable unit literal (validation normally catches these
/// first; this is the fail-fast path used by `hash`).
pub fn resolve(value: &Value, ty: &Type, env: &TypeEnv) -> Result<Value, Box<ValidationError>> {
    resolve_at(value, ty, "", env)
}

fn resolve_at(
    value: &Value,
    ty: &Type,
    path: &str,
    env: &TypeEnv,
) -> Result<Value, Box<ValidationError>> {
    match ty {
        Type::Named(n) => {
            if let Some(t) = env.resolve(n) {
                resolve_at(value, t, path, env)
            } else if env.is_unit(n) {
                resolve_unit(value, n, path, env)
            } else {
                Ok(value.clone())
            }
        }
        Type::Brand { inner, .. } => resolve_at(value, inner, path, env),

        Type::Record { fields, .. } => {
            let Value::Map(m) = value else {
                return Ok(value.clone());
            };
            let mut out = BTreeMap::new();
            for (k, v) in m {
                match fields.iter().find(|f| &f.name == k) {
                    Some(f) => {
                        out.insert(k.clone(), resolve_at(v, &f.ty, &child(path, k), env)?);
                    }
                    None => {
                        // Unknown key (validation rejects it); never forward a
                        // unit literal to the encoder — reject it here too.
                        reject_stray_unit(v, &child(path, k))?;
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            // Materialize absent defaulted fields into the canonical form
            // (§7 step 3, D18). An absent bare-optional field stays absent.
            for f in fields {
                if !out.contains_key(&f.name)
                    && let Some(def) = &f.default
                {
                    out.insert(
                        f.name.clone(),
                        resolve_at(def, &f.ty, &child(path, &f.name), env)?,
                    );
                }
            }
            Ok(Value::Map(out))
        }

        Type::Map(v_ty) => {
            let Value::Map(m) = value else {
                return Ok(value.clone());
            };
            let mut out = BTreeMap::new();
            for (k, v) in m {
                out.insert(k.clone(), resolve_at(v, v_ty, &child(path, k), env)?);
            }
            Ok(Value::Map(out))
        }

        Type::List(elem) => {
            let Value::List(xs) = value else {
                return Ok(value.clone());
            };
            let mut out = Vec::with_capacity(xs.len());
            for (i, x) in xs.iter().enumerate() {
                out.push(resolve_at(x, elem, &format!("{path}[{i}]"), env)?);
            }
            Ok(Value::List(out))
        }

        // Non-unit scalar/leaf type. A unit literal here is a kind mismatch
        // (a unit literal only belongs in a unit-typed field); reject it rather
        // than copy it forward to the encoder, which panics on an unresolved
        // unit. Other values copy through.
        _ => {
            reject_stray_unit(value, path)?;
            Ok(value.clone())
        }
    }
}

/// Error if `value` is (or contains) an unresolved unit literal — a unit
/// literal in a non-unit-typed slot. Keeps `resolve` panic-proof even for an
/// invalid document (the encoder aborts on `Value::Unit`).
fn reject_stray_unit(value: &Value, path: &str) -> Result<(), Box<ValidationError>> {
    if contains_unit(value) {
        return Err(Box::new(
            ValidationError::new(path, render(value), "a non-unit type")
                .with_failed("unit literal in a non-unit field"),
        ));
    }
    Ok(())
}

fn contains_unit(v: &Value) -> bool {
    match v {
        Value::Unit { .. } => true,
        Value::List(xs) => xs.iter().any(contains_unit),
        Value::Map(m) => m.values().any(contains_unit),
        _ => false,
    }
}

/// Resolve a unit-typed field: a unit literal becomes its base `Int`; a bare
/// base-unit integer is already canonical; anything else is copied (validation
/// reports the kind mismatch).
fn resolve_unit(
    value: &Value,
    unit: &str,
    path: &str,
    env: &TypeEnv,
) -> Result<Value, Box<ValidationError>> {
    match value {
        Value::Unit { mantissa, suffix } => {
            let base = env.resolve_unit(unit, mantissa, suffix).map_err(|msg| {
                Box::new(
                    ValidationError::new(path, render(value), unit.to_string()).with_failed(msg),
                )
            })?;
            Ok(Value::Int(base))
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::TypeEnv;
    use mangrove_core::Value;
    use mangrove_syntax::{Type, UnitDef, parse_type};
    use std::collections::BTreeMap;

    fn bytes_env() -> TypeEnv {
        TypeEnv::build(
            &[],
            &[UnitDef {
                name: "Bytes".into(),
                members: vec![
                    ("B".into(), 1.into()),
                    ("Ki".into(), 1024.into()),
                    ("Mi".into(), 1_048_576.into()),
                ],
            }],
        )
        .unwrap()
    }

    #[test]
    fn resolves_unit_field_to_base_int() {
        let env = bytes_env();
        let ty = parse_type("{ size: Bytes }").unwrap();
        let mut m = BTreeMap::new();
        m.insert(
            "size".to_string(),
            Value::Unit {
                mantissa: 512.into(),
                suffix: "Mi".into(),
            },
        );
        let resolved = resolve(&Value::Map(m), &ty, &env).unwrap();
        let Value::Map(out) = resolved else { panic!() };
        assert_eq!(out.get("size"), Some(&Value::Int(536_870_912.into())));
    }

    #[test]
    fn non_unit_value_is_copied() {
        let env = bytes_env();
        let ty = Type::Int;
        assert_eq!(
            resolve(&Value::Int(7.into()), &ty, &env).unwrap(),
            Value::Int(7.into())
        );
    }

    #[test]
    fn unit_in_non_unit_field_errors_not_panics() {
        // Was: copied through to the CBOR encoder → panic. Now a clean error.
        let env = bytes_env();
        let ty = parse_type("{ n: int }").unwrap();
        let mut m = BTreeMap::new();
        m.insert(
            "n".to_string(),
            Value::Unit {
                mantissa: 512.into(),
                suffix: "Mi".into(),
            },
        );
        assert!(resolve(&Value::Map(m), &ty, &env).is_err());
    }

    #[test]
    fn absent_default_is_materialized() {
        let env = TypeEnv::build(&[], &[]).unwrap();
        let ty = parse_type("{ n: int | *1 }").unwrap();
        let resolved = resolve(&Value::Map(BTreeMap::new()), &ty, &env).unwrap();
        let Value::Map(out) = resolved else { panic!() };
        assert_eq!(out.get("n"), Some(&Value::Int(1.into())));
    }

    #[test]
    fn absent_optional_is_not_materialized() {
        let env = TypeEnv::build(&[], &[]).unwrap();
        let ty = parse_type("{ n?: bool }").unwrap();
        let resolved = resolve(&Value::Map(BTreeMap::new()), &ty, &env).unwrap();
        let Value::Map(out) = resolved else { panic!() };
        assert!(out.is_empty());
    }

    #[test]
    fn bad_unit_suffix_errors() {
        let env = bytes_env();
        let ty = parse_type("{ size: Bytes }").unwrap();
        let mut m = BTreeMap::new();
        m.insert(
            "size".to_string(),
            Value::Unit {
                mantissa: 256.into(),
                suffix: "MB".into(),
            },
        );
        assert!(resolve(&Value::Map(m), &ty, &env).is_err());
    }
}
