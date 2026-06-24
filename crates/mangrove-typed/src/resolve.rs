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

        Type::Record { fields } => {
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
                        out.insert(k.clone(), v.clone());
                    }
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

        // Non-unit scalars/leaves: copy through. A `Value::Unit` in a non-unit
        // field is a validation error, not a resolution concern — copy it and
        // let validation report it (it will never reach the encoder for a valid
        // doc).
        _ => Ok(value.clone()),
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
