//! Validate a value against a type (spec §6 rules, §12 errors). No inference:
//! a kind mismatch is an error, never a coercion. All errors are accumulated
//! (not fail-fast), each with a dotted path.

use crate::env::TypeEnv;
use mangrove_core::Value;
use mangrove_core::error::ValidationError;
use mangrove_syntax::{Annotation, Type};
use regex::Regex;
use std::collections::HashSet;

/// Validate `value` against `ty`. An empty Vec means valid.
pub fn validate(value: &Value, ty: &Type, env: &TypeEnv) -> Vec<ValidationError> {
    check(value, ty, "", env)
}

/// Advisory `@deprecated` messages for every present field whose definition is
/// `@deprecated` (§4.9). Never errors; used by `mangrove check` for warnings.
pub fn deprecations(value: &Value, ty: &Type, env: &TypeEnv) -> Vec<String> {
    let mut out = Vec::new();
    walk_deprecations(value, ty, "", env, &mut out);
    out
}

fn walk_deprecations(value: &Value, ty: &Type, path: &str, env: &TypeEnv, out: &mut Vec<String>) {
    match ty {
        Type::Named(n) => {
            if let Some(t) = env.resolve(n) {
                walk_deprecations(value, t, path, env, out);
            }
        }
        Type::Brand { inner, .. } => walk_deprecations(value, inner, path, env, out),
        Type::Record { fields } => {
            if let Value::Map(m) = value {
                for f in fields {
                    if let Some(v) = m.get(&f.name) {
                        let p = child(path, &f.name);
                        if let Some(msg) = Annotation::find(&f.annotations, "deprecated") {
                            out.push(format!("{p}: deprecated: {msg}"));
                        }
                        walk_deprecations(v, &f.ty, &p, env, out);
                    }
                }
            }
        }
        Type::Map(vt) => {
            if let Value::Map(m) = value {
                for (k, v) in m {
                    walk_deprecations(v, vt, &child(path, k), env, out);
                }
            }
        }
        Type::List(elem) => {
            if let Value::List(xs) = value {
                for (i, x) in xs.iter().enumerate() {
                    walk_deprecations(x, elem, &format!("{path}[{i}]"), env, out);
                }
            }
        }
        _ => {}
    }
}

fn check(value: &Value, ty: &Type, path: &str, env: &TypeEnv) -> Vec<ValidationError> {
    match ty {
        Type::Int => kind(value, matches!(value, Value::Int(_)), path, "int"),
        Type::Decimal => kind(value, matches!(value, Value::Decimal(_)), path, "decimal"),
        Type::Str => kind(value, matches!(value, Value::Str(_)), path, "str"),
        Type::Bool => kind(value, matches!(value, Value::Bool(_)), path, "bool"),
        Type::Bytes => kind(value, matches!(value, Value::Bytes(_)), path, "bytes"),

        Type::IntRange { min, max } => match value {
            Value::Int(n) => {
                if let Some(mn) = min
                    && n < mn
                {
                    return vec![mismatch(path, value, ty).with_failed(format!(">= {mn}"))];
                }
                if let Some(mx) = max
                    && n > mx
                {
                    return vec![mismatch(path, value, ty).with_failed(format!("<= {mx}"))];
                }
                vec![]
            }
            _ => vec![mismatch(path, value, ty)],
        },

        Type::DecRange { min, max } => match value {
            Value::Decimal(d) => {
                if let Some(mn) = min
                    && d < mn
                {
                    return vec![mismatch(path, value, ty).with_failed(format!(">= {mn}"))];
                }
                if let Some(mx) = max
                    && d > mx
                {
                    return vec![mismatch(path, value, ty).with_failed(format!("<= {mx}"))];
                }
                vec![]
            }
            _ => vec![mismatch(path, value, ty)],
        },

        Type::StrRegex(re) => match value {
            Value::Str(s) => match Regex::new(re) {
                Ok(r) if r.is_match(s) => vec![],
                Ok(_) => vec![mismatch(path, value, ty).with_failed(format!("=~ {re:?}"))],
                Err(_) => vec![mismatch(path, value, ty).with_failed("valid regex in schema")],
            },
            _ => vec![mismatch(path, value, ty)],
        },

        Type::LitStr(lit) => match value {
            Value::Str(s) if s == lit => vec![],
            _ => vec![mismatch(path, value, ty)],
        },
        Type::LitInt(lit) => match value {
            Value::Int(n) if n == lit => vec![],
            _ => vec![mismatch(path, value, ty)],
        },
        Type::LitBool(lit) => match value {
            Value::Bool(b) if b == lit => vec![],
            _ => vec![mismatch(path, value, ty)],
        },

        Type::Record { fields } => {
            let Value::Map(m) = value else {
                return vec![mismatch(path, value, ty)];
            };
            let mut errs = Vec::new();
            let known: HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            for f in fields {
                match m.get(&f.name) {
                    Some(v) => errs.extend(check(v, &f.ty, &child(path, &f.name), env)),
                    // a defaulted field absent is valid — but the default value
                    // itself must satisfy the field type (caught at validation).
                    None if f.default.is_some() => errs.extend(check(
                        f.default.as_ref().unwrap(),
                        &f.ty,
                        &child(path, &f.name),
                        env,
                    )),
                    None if f.optional => {}
                    None => errs.push(
                        ValidationError::new(child(path, &f.name), "absent", render_type(&f.ty))
                            .with_failed("required field missing"),
                    ),
                }
            }
            for (k, v) in m {
                if !known.contains(k.as_str()) {
                    errs.push(
                        ValidationError::new(child(path, k), render(v), "(no such field)")
                            .with_failed("unknown field"),
                    );
                }
            }
            errs
        }

        Type::Map(v_ty) => {
            let Value::Map(m) = value else {
                return vec![mismatch(path, value, ty)];
            };
            let mut errs = Vec::new();
            for (k, v) in m {
                errs.extend(check(v, v_ty, &child(path, k), env));
            }
            errs
        }

        Type::List(elem) => {
            let Value::List(xs) = value else {
                return vec![mismatch(path, value, ty)];
            };
            let mut errs = Vec::new();
            for (i, x) in xs.iter().enumerate() {
                errs.extend(check(x, elem, &format!("{path}[{i}]"), env));
            }
            errs
        }

        Type::Union(variants) => {
            if variants
                .iter()
                .any(|v| check(value, v, path, env).is_empty())
            {
                vec![]
            } else {
                vec![mismatch(path, value, ty).with_failed("no matching variant")]
            }
        }

        Type::Named(n) => {
            if let Some(t) = env.resolve(n) {
                let mut sub = check(value, t, path, env);
                // A named type's @message overrides the default message on its
                // own failures (§4.9, §12).
                if let Some(msg) = env.message(n) {
                    for e in &mut sub {
                        if e.message.is_none() {
                            e.message = Some(msg.to_string());
                        }
                    }
                }
                sub
            } else if env.is_unit(n) {
                check_unit(value, n, path, env)
            } else {
                vec![
                    ValidationError::new(path, render(value), n.clone())
                        .with_failed("unknown type"),
                ]
            }
        }

        // §4.6: a brand validates exactly as its structural `inner` — a bare
        // literal into a brand-typed slot is auto-constructed (no ceremony).
        Type::Brand { inner, .. } => check(value, inner, path, env),
    }
}

/// Validate a value against a unit type `unit` (§4.5): a unit literal must
/// resolve (suffix is a member, exact-integer base); a bare base-unit integer
/// is accepted; any other kind is a mismatch.
fn check_unit(value: &Value, unit: &str, path: &str, env: &TypeEnv) -> Vec<ValidationError> {
    match value {
        Value::Unit { mantissa, suffix } => match env.resolve_unit(unit, mantissa, suffix) {
            Ok(_) => vec![],
            Err(msg) => {
                vec![ValidationError::new(path, render(value), unit.to_string()).with_failed(msg)]
            }
        },
        Value::Int(_) => vec![],
        _ => vec![ValidationError::new(path, render(value), unit.to_string())],
    }
}

fn kind(value: &Value, ok: bool, path: &str, expected: &str) -> Vec<ValidationError> {
    if ok {
        vec![]
    } else {
        vec![ValidationError::new(path, render(value), expected)]
    }
}

fn mismatch(path: &str, value: &Value, ty: &Type) -> ValidationError {
    ValidationError::new(path, render(value), render_type(ty))
}

pub(crate) fn child(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

pub(crate) fn render(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Str(s) => format!("{s:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Bytes(_) => "<bytes>".into(),
        Value::List(_) => "<list>".into(),
        Value::Map(_) => "<map>".into(),
        Value::Unit { mantissa, suffix } => format!("{mantissa}{suffix}"),
    }
}

fn render_type(ty: &Type) -> String {
    match ty {
        Type::Int => "int".into(),
        Type::Decimal => "decimal".into(),
        Type::Str => "str".into(),
        Type::Bool => "bool".into(),
        Type::Bytes => "bytes".into(),
        Type::IntRange { min, max } => render_range("int", min, max),
        Type::DecRange { min, max } => render_range("decimal", min, max),
        Type::StrRegex(re) => format!("str & =~ {re:?}"),
        Type::LitStr(s) => format!("{s:?}"),
        Type::LitInt(n) => n.to_string(),
        Type::LitBool(b) => b.to_string(),
        Type::Record { fields } => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            format!("{{ {} }}", names.join(", "))
        }
        Type::Map(v) => format!("{{ [str]: {} }}", render_type(v)),
        Type::List(t) => format!("[ {} ]", render_type(t)),
        Type::Union(vs) => vs.iter().map(render_type).collect::<Vec<_>>().join(" | "),
        Type::Named(n) => n.clone(),
        Type::Brand { name, inner } => {
            if name.is_empty() {
                format!("brand {}", render_type(inner))
            } else {
                name.clone()
            }
        }
    }
}

fn render_range<T: std::fmt::Display>(base: &str, min: &Option<T>, max: &Option<T>) -> String {
    let mut s = base.to_string();
    if let Some(mn) = min {
        s.push_str(&format!(" & >= {mn}"));
    }
    if let Some(mx) = max {
        s.push_str(&format!(" & <= {mx}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::TypeEnv;
    use mangrove_core::Value;
    use mangrove_syntax::{Type, parse_type};

    fn ty(s: &str) -> Type {
        parse_type(s).unwrap()
    }
    fn env() -> TypeEnv {
        TypeEnv::build(&[], &[]).unwrap()
    }
    fn errs(v: Value, t: &str) -> Vec<mangrove_core::error::ValidationError> {
        validate(&v, &ty(t), &env())
    }
    fn map(pairs: &[(&str, Value)]) -> Value {
        Value::Map(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn int_in_range_ok() {
        assert!(errs(Value::Int(5.into()), "int & >= 1 & <= 10").is_empty());
    }
    #[test]
    fn int_out_of_range_errs() {
        let e = errs(Value::Int(70000.into()), "int & >= 1 & <= 65535");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].failed.as_deref(), Some("<= 65535"));
    }
    #[test]
    fn kind_mismatch_errs_no_coercion() {
        let e = errs(Value::Str("5".into()), "int");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].expected, "int");
    }
    #[test]
    fn regex_match_and_miss() {
        assert!(errs(Value::Str("abc".into()), "str & =~ \"^[a-z]+$\"").is_empty());
        assert_eq!(
            errs(Value::Str("A".into()), "str & =~ \"^[a-z]+$\"").len(),
            1
        );
    }
    #[test]
    fn union_membership() {
        assert!(errs(Value::Str("dev".into()), "\"dev\" | \"prod\"").is_empty());
        assert_eq!(errs(Value::Str("x".into()), "\"dev\" | \"prod\"").len(), 1);
    }
    #[test]
    fn record_missing_required_and_unknown_key() {
        let v = map(&[
            ("host", Value::Str("h".into())),
            ("extra", Value::Bool(true)),
        ]);
        let e = errs(v, "{ host: str, port: int }");
        assert_eq!(e.len(), 2); // missing `port` + unknown `extra`
    }
    #[test]
    fn optional_absent_ok_and_present_checked() {
        let v = map(&[("host", Value::Str("h".into()))]);
        assert!(validate(&v, &ty("{ host: str, tls?: bool }"), &env()).is_empty());
        let bad = map(&[
            ("host", Value::Str("h".into())),
            ("tls", Value::Int(1.into())),
        ]);
        assert_eq!(
            validate(&bad, &ty("{ host: str, tls?: bool }"), &env()).len(),
            1
        );
    }
    #[test]
    fn nested_path_reported() {
        let inner = map(&[("port", Value::Int(70000.into()))]);
        let v = map(&[("listen", inner)]);
        let e = validate(&v, &ty("{ listen: { port: int & <= 65535 } }"), &env());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "listen.port");
    }
    #[test]
    fn list_element_error() {
        let l = Value::List(vec![Value::Int(1.into()), Value::Str("x".into())]);
        let e = validate(&l, &ty("[ int ]"), &env());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "[1]");
    }
    #[test]
    fn map_value_error() {
        let v = map(&[("a", Value::Int(1.into())), ("b", Value::Str("x".into()))]);
        let e = validate(&v, &ty("{ [str]: int }"), &env());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "b");
    }

    fn bytes_env() -> TypeEnv {
        TypeEnv::build(
            &[],
            &[mangrove_syntax::UnitDef {
                name: "Bytes".into(),
                members: vec![("B".into(), 1.into()), ("Mi".into(), 1_048_576.into())],
            }],
        )
        .unwrap()
    }

    #[test]
    fn unit_literal_validates_against_unit_type() {
        let env = bytes_env();
        let v = Value::Unit {
            mantissa: 512.into(),
            suffix: "Mi".into(),
        };
        assert!(validate(&v, &Type::Named("Bytes".into()), &env).is_empty());
    }

    #[test]
    fn wrong_unit_suffix_errors() {
        let env = bytes_env();
        let v = Value::Unit {
            mantissa: 1.into(),
            suffix: "core".into(),
        };
        let e = validate(&v, &Type::Named("Bytes".into()), &env);
        assert_eq!(e.len(), 1);
        assert!(e[0].failed.as_deref().unwrap().contains("core"));
    }

    #[test]
    fn brand_validates_against_inner() {
        assert!(errs(Value::Int(21000.into()), "brand int & >= 0").is_empty());
        assert_eq!(errs(Value::Int((-1).into()), "brand int & >= 0").len(), 1);
    }

    #[test]
    fn message_annotation_surfaces_in_error() {
        use mangrove_syntax::{Annotation, TypeDef, parse_type};
        let env = TypeEnv::build(
            &[TypeDef {
                name: "Port".into(),
                ty: parse_type("int & >= 1 & <= 65535").unwrap(),
                annotations: vec![Annotation {
                    name: "message".into(),
                    arg: Some("bad port".into()),
                }],
            }],
            &[],
        )
        .unwrap();
        let e = validate(&Value::Int(70000.into()), &Type::Named("Port".into()), &env);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].message.as_deref(), Some("bad port"));
    }

    #[test]
    fn deprecated_field_yields_advisory() {
        let ty = ty("{ image: str @deprecated(\"use image_ref\") }");
        let v = map(&[("image", Value::Str("x".into()))]);
        let warns = super::deprecations(&v, &ty, &env());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("image") && warns[0].contains("image_ref"));
    }

    #[test]
    fn absent_defaulted_field_is_valid() {
        let v = map(&[]);
        assert!(validate(&v, &ty("{ n: int | *1 }"), &env()).is_empty());
    }

    #[test]
    fn ill_typed_default_errors() {
        // default 0 violates >= 1
        let v = map(&[]);
        assert_eq!(validate(&v, &ty("{ n: int & >= 1 | *0 }"), &env()).len(), 1);
    }
}
