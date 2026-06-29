//! Validate a value against a type (spec §6 rules, §12 errors). No inference:
//! a kind mismatch is an error, never a coercion. All errors are accumulated
//! (not fail-fast), each with a dotted path.

use crate::env::TypeEnv;
use mangrove_core::Value;
use mangrove_core::error::ValidationError;
use mangrove_syntax::{Annotation, Require, Type};
use regex::Regex;
use std::collections::HashSet;

/// Validate `value` against `ty`. An empty Vec means valid.
///
/// ```
/// use mangrove_core::Value;
/// use mangrove_syntax::Type;
/// use mangrove_typed::{TypeEnv, validate};
///
/// let env = TypeEnv::build(&[], &[]).unwrap();
/// assert!(validate(&Value::Int(5.into()), &Type::Int, &env).is_empty());
/// assert!(!validate(&Value::Str("x".into()), &Type::Int, &env).is_empty());
/// ```
pub fn validate(value: &Value, ty: &Type, env: &TypeEnv) -> Vec<ValidationError> {
    check(value, ty, "", env, 0)
}

/// Bound on validation recursion. It counts every type node crossed (value
/// descent *and* name resolution), so it's a function of value depth × the
/// type-graph fan-out per level — not value depth alone. Realistic recursive
/// types stay well under it (e.g. arbitrary-JSON, multiplier ~3, tops out near
/// 375 at the parser's 128 nesting cap). A *pathologically* long alias chain
/// (`A=B=…`, dozens deep) could inflate the multiplier enough to reject a shallow
/// value — contrived, not seen in real schemas. The guard's job is termination:
/// it fires before the 8 MB main-thread stack does, turning a runaway into a
/// clean error instead of a crash.
const MAX_DEPTH: usize = 512;

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
        Type::Record { fields, .. } => {
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

fn check(
    value: &Value,
    ty: &Type,
    path: &str,
    env: &TypeEnv,
    depth: usize,
) -> Vec<ValidationError> {
    if depth >= MAX_DEPTH {
        return vec![
            ValidationError::new(path, render(value), "a bounded-depth value")
                .with_failed("type nesting too deep"),
        ];
    }
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

        Type::Record { fields, requires } => {
            let Value::Map(m) = value else {
                return vec![mismatch(path, value, ty)];
            };
            let mut errs = Vec::new();
            let known: HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            for f in fields {
                match m.get(&f.name) {
                    Some(v) => errs.extend(check(v, &f.ty, &child(path, &f.name), env, depth + 1)),
                    // a defaulted field absent is valid — but the default value
                    // itself must satisfy the field type (caught at validation).
                    None if f.default.is_some() => errs.extend(check(
                        f.default.as_ref().unwrap(),
                        &f.ty,
                        &child(path, &f.name),
                        env,
                        depth + 1,
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
            // Cross-field predicates (§4.7), evaluated against the concrete
            // record. Only run when the fields themselves validated, so a
            // require doesn't fire on a missing/ill-typed operand.
            if errs.is_empty() {
                for r in requires {
                    match crate::predicate::eval_pred(&r.pred, m) {
                        Ok(true) => {}
                        Ok(false) => errs.push(require_error(path, r)),
                        Err(_) => errs.push(require_error(path, r)),
                    }
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
                errs.extend(check(v, v_ty, &child(path, k), env, depth + 1));
            }
            errs
        }

        Type::List(elem) => {
            let Value::List(xs) = value else {
                return vec![mismatch(path, value, ty)];
            };
            let mut errs = Vec::new();
            for (i, x) in xs.iter().enumerate() {
                errs.extend(check(x, elem, &format!("{path}[{i}]"), env, depth + 1));
            }
            errs
        }

        Type::Union(variants) => {
            // Attempt discriminated-union dispatch first (purely additive — does not
            // change the accept/reject set for non-DU shapes). If detection fails at
            // any step, fall through to the existing try-each-variant loop below.
            if let Some(result) = try_du_dispatch(value, variants, ty, path, env, depth) {
                return result;
            }

            // Fallback: try each variant in order, return empty on first match.
            // If an arm hits the depth guard, surface that real cause rather than
            // the generic "no matching variant" (which would otherwise mask it).
            let mut too_deep = None;
            for v in variants {
                let errs = check(value, v, path, env, depth + 1);
                if errs.is_empty() {
                    return vec![];
                }
                if too_deep.is_none()
                    && errs
                        .iter()
                        .any(|e| e.failed.as_deref() == Some("type nesting too deep"))
                {
                    too_deep = Some(errs);
                }
            }
            too_deep.unwrap_or_else(|| {
                vec![mismatch(path, value, ty).with_failed("no matching variant")]
            })
        }

        Type::Named(n) => {
            if let Some(t) = env.resolve(n) {
                let mut sub = check(value, t, path, env, depth + 1);
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
        Type::Brand { inner, .. } => check(value, inner, path, env, depth + 1),
    }
}

/// Try discriminated-union dispatch. Returns `Some(errors)` when DU detection
/// succeeds and we've dispatched to a single variant (errors may be empty on
/// success). Returns `None` to signal the caller to fall back to try-each.
///
/// Detection rules (all must hold or we return None):
///   1. Every variant resolves through `Named`/`Brand` to a plain `Record`.
///   2. There exists a field name that is: present in every resolved record,
///      non-optional, has a literal type (`LitStr`/`LitInt`/`LitBool`), AND
///      the literal values are pairwise distinct.
///   3. The value is a `Value::Map` that contains the discriminant field.
///   4. The field's value matches exactly one variant's literal.
fn try_du_dispatch(
    value: &Value,
    variants: &[Type],
    union_ty: &Type,
    path: &str,
    env: &TypeEnv,
    depth: usize,
) -> Option<Vec<ValidationError>> {
    use mangrove_syntax::FieldDef;

    // Step 1: resolve each variant to a Record, keeping the original type node.
    let resolved: Vec<(&Type, &[FieldDef])> = variants
        .iter()
        .map(|orig| resolve_to_record(orig, env).map(|fields| (orig, fields)))
        .collect::<Option<Vec<_>>>()?;

    // Step 2: find a qualifying discriminant field.
    // Iterate candidate names in the order of the first variant's fields.
    let disc_name: &str = resolved[0]
        .1
        .iter()
        .find(|f| is_discriminant(f, &resolved))?
        .name
        .as_str();

    // Step 3: value must be a map containing the discriminant field.
    let Value::Map(map) = value else {
        return None;
    };
    let disc_val = map.get(disc_name)?;

    // Step 4: find the single matching variant.
    let matched = resolved.iter().find(|(_, fields)| {
        fields
            .iter()
            .find(|f| f.name == disc_name)
            .is_some_and(|f| lit_matches(&f.ty, disc_val))
    });

    Some(match matched {
        Some((orig_ty, _)) => check(value, orig_ty, path, env, depth + 1),
        None => {
            // Discriminant present but value doesn't match any variant.
            let candidates: Vec<String> = resolved
                .iter()
                .filter_map(|(_, fields)| {
                    fields
                        .iter()
                        .find(|f| f.name == disc_name)
                        .map(|f| render_type(&f.ty))
                })
                .collect();
            let failed = format!(
                "unknown {disc_name} {}: expected one of {}",
                render(disc_val),
                candidates.join(" | ")
            );
            vec![mismatch(path, value, union_ty).with_failed(failed)]
        }
    })
}

/// Resolve a type through `Named` (one level of env lookup) and `Brand` wrappers
/// until we reach a `Record`. Returns `None` if the chain doesn't end at a Record.
fn resolve_to_record<'a>(
    ty: &'a Type,
    env: &'a TypeEnv,
) -> Option<&'a [mangrove_syntax::FieldDef]> {
    match ty {
        Type::Record { fields, .. } => Some(fields),
        Type::Brand { inner, .. } => resolve_to_record(inner, env),
        Type::Named(n) => resolve_to_record(env.resolve(n)?, env),
        _ => None,
    }
}

/// Returns `true` if `field` qualifies as a discriminant field given all resolved
/// variant records. A qualifying field must, in every variant, be: present,
/// non-optional, have a literal type, and the literal values must be pairwise distinct.
fn is_discriminant(
    candidate: &mangrove_syntax::FieldDef,
    resolved: &[(&Type, &[mangrove_syntax::FieldDef])],
) -> bool {
    if candidate.optional {
        return false;
    }
    if !matches!(
        candidate.ty,
        Type::LitStr(_) | Type::LitInt(_) | Type::LitBool(_)
    ) {
        return false;
    }

    // Every variant must have this field as a non-optional literal, and all
    // literals must be pairwise distinct.
    let mut seen_lits: Vec<&Type> = Vec::new();
    for (_, fields) in resolved {
        let Some(f) = fields.iter().find(|f| f.name == candidate.name) else {
            return false;
        };
        if f.optional {
            return false;
        }
        if !matches!(f.ty, Type::LitStr(_) | Type::LitInt(_) | Type::LitBool(_)) {
            return false;
        }
        // Check distinctness against previously seen literals.
        if seen_lits.iter().any(|prev| lit_eq(prev, &f.ty)) {
            return false;
        }
        seen_lits.push(&f.ty);
    }
    true
}

/// Returns `true` when `value` equals the literal encoded in `lit_ty`.
fn lit_matches(lit_ty: &Type, value: &Value) -> bool {
    match (lit_ty, value) {
        (Type::LitStr(s), Value::Str(v)) => s == v,
        (Type::LitInt(n), Value::Int(v)) => n == v,
        (Type::LitBool(b), Value::Bool(v)) => b == v,
        _ => false,
    }
}

/// Returns `true` when two literal types encode the same value.
fn lit_eq(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::LitStr(x), Type::LitStr(y)) => x == y,
        (Type::LitInt(x), Type::LitInt(y)) => x == y,
        (Type::LitBool(x), Type::LitBool(y)) => x == y,
        _ => false,
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

fn require_error(path: &str, r: &Require) -> ValidationError {
    let mut e = ValidationError::new(path, "<record>", "a require predicate that holds")
        .with_failed("require");
    e.message = r.message.clone();
    e
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
        Value::Unset => "unset".into(),
        Value::Ref(name) => name.clone(),
        Value::Interp(_) => "<interpolated string>".into(),
        Value::Match { .. } => "<match>".into(),
        Value::Call { name, .. } => format!("{name}(…)"),
        Value::ModuleCall { alias, .. } => format!("{alias}(…)"),
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
        Type::Record { fields, .. } => {
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
    fn require_passes_and_fails_with_message() {
        let t = "{ a: int, b: int, require: a <= b @message(\"a must be <= b\") }";
        assert!(
            validate(
                &map(&[("a", Value::Int(1.into())), ("b", Value::Int(2.into()))]),
                &ty(t),
                &env()
            )
            .is_empty()
        );
        let e = validate(
            &map(&[("a", Value::Int(5.into())), ("b", Value::Int(2.into()))]),
            &ty(t),
            &env(),
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].message.as_deref(), Some("a must be <= b"));
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

    fn d(s: &str) -> Value {
        Value::Decimal(s.parse().unwrap())
    }

    #[test]
    fn decimal_range_bounds_and_kind() {
        assert!(errs(d("0.5"), "decimal & >= 0.0 & <= 1.0").is_empty());
        assert_eq!(errs(d("-0.1"), "decimal & >= 0.0").len(), 1); // below min
        assert_eq!(errs(d("2.0"), "decimal & <= 1.0").len(), 1); // above max
        assert_eq!(errs(Value::Int(1.into()), "decimal & >= 0.0").len(), 1); // wrong kind
    }

    #[test]
    fn literal_type_mismatches() {
        assert!(errs(Value::Int(1.into()), "1").is_empty());
        assert!(errs(Value::Bool(true), "true").is_empty());
        assert_eq!(errs(Value::Int(2.into()), "1").len(), 1); // LitInt miss
        assert_eq!(errs(Value::Bool(false), "true").len(), 1); // LitBool miss
        assert_eq!(errs(Value::Str("y".into()), "\"x\"").len(), 1); // LitStr miss
    }

    #[test]
    fn container_kind_mismatches() {
        assert_eq!(errs(Value::Int(1.into()), "{ a: int }").len(), 1); // record vs non-map
        assert_eq!(errs(Value::Int(1.into()), "[ int ]").len(), 1); // list vs non-list
        assert_eq!(errs(Value::Int(1.into()), "{ [str]: int }").len(), 1); // map vs non-map
    }

    #[test]
    fn unit_type_accepts_bare_int_rejects_other_kinds() {
        let env = bytes_env();
        // a bare base-unit integer is accepted into a unit-typed slot (§4.5)
        assert!(validate(&Value::Int(42.into()), &Type::Named("Bytes".into()), &env).is_empty());
        // any other kind is a mismatch
        assert_eq!(
            validate(&Value::Str("x".into()), &Type::Named("Bytes".into()), &env).len(),
            1
        );
    }

    #[test]
    fn unknown_named_type_errors() {
        let e = validate(&Value::Int(1.into()), &Type::Named("Nope".into()), &env());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].failed.as_deref(), Some("unknown type"));
    }

    fn json_env() -> TypeEnv {
        // Json = str | int | [Json] | { [str]: Json }
        let json = Type::Union(vec![
            Type::Str,
            Type::Int,
            Type::List(Box::new(Type::Named("Json".into()))),
            Type::Map(Box::new(Type::Named("Json".into()))),
        ]);
        TypeEnv::build(
            &[mangrove_syntax::TypeDef {
                name: "Json".into(),
                ty: json,
                annotations: vec![],
            }],
            &[],
        )
        .unwrap()
    }

    #[test]
    fn recursive_type_validates_nested_value() {
        let env = json_env();
        let j = Type::Named("Json".into());
        // { a: [ 1, "x", { b: 2 } ] } — nested maps/lists/scalars all validate
        let nested = map(&[(
            "a",
            Value::List(vec![
                Value::Int(1.into()),
                Value::Str("x".into()),
                map(&[("b", Value::Int(2.into()))]),
            ]),
        )]);
        assert!(validate(&nested, &j, &env).is_empty());
        // bool is not a Json variant here → rejected (no coercion)
        assert!(!validate(&Value::Bool(true), &j, &env).is_empty());
    }

    #[test]
    fn validation_depth_is_bounded_not_overflowing() {
        // `type L = [L]` against a value nested far past the guard returns a clean
        // error rather than recursing unboundedly. Run on a generous stack (the
        // guard at MAX_DEPTH is sized for the 8 MB main thread, not the 2 MB
        // default test thread).
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let env = TypeEnv::build(
                    &[mangrove_syntax::TypeDef {
                        name: "L".into(),
                        ty: Type::List(Box::new(Type::Named("L".into()))),
                        annotations: vec![],
                    }],
                    &[],
                )
                .unwrap();
                let mut v = Value::List(vec![]);
                for _ in 0..1200 {
                    v = Value::List(vec![v]);
                }
                let e = validate(&v, &Type::Named("L".into()), &env);
                assert!(
                    e.iter()
                        .any(|x| x.failed.as_deref() == Some("type nesting too deep")),
                    "expected a depth-guard error"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn over_deep_union_surfaces_depth_error() {
        // A too-deep value against a recursive *union* (Json) must report the real
        // cause ("type nesting too deep"), not the generic "no matching variant".
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let env = json_env();
                let mut v = Value::Int(1.into());
                for _ in 0..400 {
                    v = map(&[("k", v)]);
                }
                let e = validate(&v, &Type::Named("Json".into()), &env);
                assert!(
                    e.iter()
                        .any(|x| x.failed.as_deref() == Some("type nesting too deep")),
                    "expected a depth error, got {:?}",
                    e.iter().map(|x| x.failed.clone()).collect::<Vec<_>>()
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn invalid_schema_regex_errors() {
        // `[` is an unclosed character class — invalid regex compiled at validate time
        let e = errs(Value::Str("x".into()), "str & =~ \"[\"");
        assert_eq!(e.len(), 1);
        assert!(e[0].failed.as_deref().unwrap().contains("valid regex"));
    }

    // --- discriminated-union tests ---

    fn du_env() -> TypeEnv {
        // Resource = { kind: "PVC",     spec: { storage: str } }
        //           | { kind: "CronJob", schedule: str }
        use mangrove_syntax::{TypeDef, parse_type};
        TypeEnv::build(
            &[TypeDef {
                name: "Resource".into(),
                ty: parse_type(
                    "{ kind: \"PVC\", spec: { storage: str } } | { kind: \"CronJob\", schedule: str }",
                )
                .unwrap(),
                annotations: vec![],
            }],
            &[],
        )
        .unwrap()
    }

    #[test]
    fn du_precise_nested_error() {
        // PVC with wrong storage type — error must be at spec.storage, not "no matching variant"
        let env = du_env();
        let v = map(&[
            ("kind", Value::Str("PVC".into())),
            ("spec", map(&[("storage", Value::Int(123.into()))])),
        ]);
        let e = validate(&v, &Type::Named("Resource".into()), &env);
        assert_eq!(e.len(), 1, "expected exactly one error, got: {e:?}");
        assert_eq!(
            e[0].path, "spec.storage",
            "error must point at spec.storage"
        );
        // must be a type mismatch, not "no matching variant"
        assert_ne!(
            e[0].failed.as_deref(),
            Some("no matching variant"),
            "must be a precise error, not generic union failure"
        );
    }

    #[test]
    fn du_unknown_discriminant_value() {
        // kind: "Service" — no variant matches; expect single error listing valid values
        let env = du_env();
        let v = map(&[
            ("kind", Value::Str("Service".into())),
            ("spec", map(&[("storage", Value::Str("1Ti".into()))])),
        ]);
        let e = validate(&v, &Type::Named("Resource".into()), &env);
        assert_eq!(e.len(), 1, "expected exactly one error, got: {e:?}");
        let failed = e[0].failed.as_deref().unwrap_or("");
        assert!(
            failed.contains("unknown kind"),
            "failed should say 'unknown kind', got: {failed:?}"
        );
        assert!(
            failed.contains("\"PVC\"") && failed.contains("\"CronJob\""),
            "failed should list valid discriminant values, got: {failed:?}"
        );
    }

    #[test]
    fn du_list_of_resources() {
        // [ PVC-ok, CronJob-bad ] — exactly one error at [1].schedule
        let env = du_env();
        let pvc = map(&[
            ("kind", Value::Str("PVC".into())),
            ("spec", map(&[("storage", Value::Str("12Ti".into()))])),
        ]);
        let cronjob_bad = map(&[
            ("kind", Value::Str("CronJob".into())),
            ("schedule", Value::Int(5.into())), // wrong type
        ]);
        let list = Value::List(vec![pvc, cronjob_bad]);
        let list_ty = Type::List(Box::new(Type::Named("Resource".into())));
        let e = validate(&list, &list_ty, &env);
        assert_eq!(e.len(), 1, "expected exactly one error, got: {e:?}");
        assert_eq!(e[0].path, "[1].schedule");
    }

    #[test]
    fn du_happy_path() {
        // Both well-formed variants should validate with zero errors
        let env = du_env();
        let pvc = map(&[
            ("kind", Value::Str("PVC".into())),
            ("spec", map(&[("storage", Value::Str("10Gi".into()))])),
        ]);
        let cronjob = map(&[
            ("kind", Value::Str("CronJob".into())),
            ("schedule", Value::Str("0 * * * *".into())),
        ]);
        assert!(
            validate(&pvc, &Type::Named("Resource".into()), &env).is_empty(),
            "PVC should be valid"
        );
        assert!(
            validate(&cronjob, &Type::Named("Resource".into()), &env).is_empty(),
            "CronJob should be valid"
        );
    }

    #[test]
    fn du_regression_non_record_union_falls_back() {
        // int | str: not a DU; value `true` should still give "no matching variant"
        let e = errs(Value::Bool(true), "int | str");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].failed.as_deref(), Some("no matching variant"));
    }

    #[test]
    fn du_regression_no_common_discriminant_falls_back() {
        // Two records with no common required literal field — must fall back to try-each.
        // A value matching the first variant should be accepted (zero errors).
        let v = map(&[("x", Value::Int(1.into()))]);
        // { x: int } | { y: str } — no common literal discriminant
        assert!(errs(v, "{ x: int } | { y: str }").is_empty());
    }

    #[test]
    fn du_named_variant_message_propagates() {
        // When a named type has @message, that message should still appear on mismatch
        // after DU dispatches to the named variant.
        use mangrove_syntax::{Annotation, TypeDef, parse_type};
        let env = TypeEnv::build(
            &[
                TypeDef {
                    name: "PVC".into(),
                    ty: parse_type("{ kind: \"PVC\", storage: str }").unwrap(),
                    annotations: vec![Annotation {
                        name: "message".into(),
                        arg: Some("invalid PVC".into()),
                    }],
                },
                TypeDef {
                    name: "CronJob".into(),
                    ty: parse_type("{ kind: \"CronJob\", schedule: str }").unwrap(),
                    annotations: vec![],
                },
                TypeDef {
                    name: "Resource".into(),
                    ty: Type::Union(vec![
                        Type::Named("PVC".into()),
                        Type::Named("CronJob".into()),
                    ]),
                    annotations: vec![],
                },
            ],
            &[],
        )
        .unwrap();
        // kind: "PVC" but storage is int — DU should dispatch to PVC, Named wraps it,
        // @message "invalid PVC" should appear on the error.
        let v = map(&[
            ("kind", Value::Str("PVC".into())),
            ("storage", Value::Int(99.into())),
        ]);
        let e = validate(&v, &Type::Named("Resource".into()), &env);
        assert_eq!(e.len(), 1, "expected one error, got: {e:?}");
        assert_eq!(
            e[0].message.as_deref(),
            Some("invalid PVC"),
            "@message from named variant should propagate"
        );
    }
}
