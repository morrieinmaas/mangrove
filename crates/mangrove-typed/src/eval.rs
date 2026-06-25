//! The L3 eval stage (§6): reduce expression markers in a composed body to plain
//! values against the param/binding environment. M4a covers `params` (§6.1) and
//! bare-name references (`Value::Ref`). It runs after compose and before
//! validate/resolve/hash, so the canonical form never contains an expression
//! (D35). An L0–L2 body has no markers and passes through unchanged.

use crate::env::TypeEnv;
use crate::validate::validate;
use mangrove_core::error::ValidationError;
use mangrove_core::{StrPart, Value};
use mangrove_syntax::{Param, Type};
use std::collections::BTreeMap;

/// Eval context: the value scope (params + sibling bindings), the params' declared
/// types (for `match` exhaustiveness, D37), and the type environment (to resolve
/// a named param type to its union).
struct Ctx<'a> {
    scope: BTreeMap<String, Value>,
    ptypes: BTreeMap<String, Type>,
    types: &'a TypeEnv,
}

/// Bound on reference-chain depth — a ref resolving to a ref … guards against a
/// cyclic binding (`a: b`, `b: a`) overflowing the stack, like every other
/// recursive pass.
const MAX_DEPTH: usize = 128;

fn err(
    path: impl Into<String>,
    got: impl Into<String>,
    expected: impl Into<String>,
) -> Box<ValidationError> {
    Box::new(ValidationError::new(path, got, expected))
}

/// Reduce every `Value::Ref` in `body` against the params (defaults bound per
/// D34) and the body's own top-level bindings (§6.1). A required param with no
/// value, or a reference to an unknown name, is a hard error.
pub fn eval(
    body: &Value,
    params: &[Param],
    types: &TypeEnv,
) -> Result<Value, Box<ValidationError>> {
    let mut scope: BTreeMap<String, Value> = BTreeMap::new();
    let mut ptypes: BTreeMap<String, Type> = BTreeMap::new();

    // Params first (they take precedence over sibling bindings, D36).
    for p in params {
        ptypes.insert(p.name.clone(), p.ty.clone());
        match &p.default {
            // Optional: bind the default. Validate a literal default against its
            // declared type so `params { n: int = "x" }` fails at the param, not
            // mysteriously downstream. (A default that itself contains a Ref is
            // checked after reduction, via the body.)
            Some(def) => {
                if !contains_ref(def) {
                    if let Some(e) = validate(def, &p.ty, types).into_iter().next() {
                        return Err(err(format!("params.{}", p.name), e.got, e.expected));
                    }
                }
                scope.insert(p.name.clone(), def.clone());
            }
            // Required and unsupplied: the document is a function, not a value.
            None => {
                return Err(err(
                    format!("params.{}", p.name),
                    "(unbound)",
                    "a value (required param — supply it via a module call)",
                ));
            }
        }
    }

    // Sibling top-level bindings, so one field can reference another (§6.1).
    if let Value::Map(m) = body {
        for (k, v) in m {
            scope.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    let cx = Ctx {
        scope,
        ptypes,
        types,
    };
    reduce(body, &cx, 0)
}

fn reduce(v: &Value, cx: &Ctx, depth: usize) -> Result<Value, Box<ValidationError>> {
    if depth >= MAX_DEPTH {
        return Err(err("", "a reference cycle", "an acyclic reference"));
    }
    match v {
        Value::Ref(name) => {
            let target = lookup(cx, name)?;
            // Resolve transitively so a binding that is itself a reference works.
            reduce(target, cx, depth + 1)
        }
        Value::Interp(parts) => {
            let mut s = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(t) => s.push_str(t),
                    StrPart::Ref(name) => {
                        let target = lookup(cx, name)?;
                        let v = reduce(target, cx, depth + 1)?;
                        s.push_str(&render_scalar(&v)?);
                    }
                }
            }
            Ok(Value::Str(s))
        }
        Value::Match { scrutinee, arms } => {
            check_match_exhaustive(scrutinee, arms, cx)?;
            let sval = reduce(scrutinee, cx, depth + 1)?;
            for (pat, val) in arms {
                let matched = match pat {
                    None => true,              // `_` wildcard
                    Some(lit) => *lit == sval, // literal pattern
                };
                if matched {
                    return reduce(val, cx, depth + 1);
                }
            }
            Err(err(
                "",
                format!(
                    "match scrutinee {}",
                    render_scalar(&sval).unwrap_or_else(|_| "value".into())
                ),
                "a value covered by a match arm",
            ))
        }
        Value::List(xs) => Ok(Value::List(
            xs.iter()
                .map(|x| reduce(x, cx, depth + 1))
                .collect::<Result<_, _>>()?,
        )),
        Value::Map(m) => {
            let mut out = BTreeMap::new();
            for (k, val) in m {
                out.insert(k.clone(), reduce(val, cx, depth + 1)?);
            }
            Ok(Value::Map(out))
        }
        other => Ok(other.clone()),
    }
}

/// Look up a name in the value scope.
fn lookup<'a>(cx: &'a Ctx, name: &str) -> Result<&'a Value, Box<ValidationError>> {
    cx.scope.get(name).ok_or_else(|| {
        err(
            "",
            format!("reference `{name}`"),
            "a param or sibling binding",
        )
    })
}

/// A `match` is total (D37): exhaustive if it has a `_` arm, or if the scrutinee
/// is a param whose type is a finite literal union that the arms fully cover.
fn check_match_exhaustive(
    scrutinee: &Value,
    arms: &[(Option<Value>, Value)],
    cx: &Ctx,
) -> Result<(), Box<ValidationError>> {
    if arms.iter().any(|(p, _)| p.is_none()) {
        return Ok(()); // has a `_` wildcard
    }
    if let Value::Ref(name) = scrutinee
        && let Some(ty) = cx.ptypes.get(name)
        && let Some(members) = literal_union_members(ty, cx.types)
    {
        let covered: Vec<&Value> = arms.iter().filter_map(|(p, _)| p.as_ref()).collect();
        let missing: Vec<&Value> = members.iter().filter(|m| !covered.contains(m)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        return Err(err(
            "",
            format!("a match missing {} arm(s)", missing.len()),
            "an arm for every union member (or a `_` arm)",
        ));
    }
    Err(err(
        "",
        "a non-exhaustive match",
        "an exhaustive match (add a `_` arm)",
    ))
}

/// The literal members of a finite literal union (resolving a named type), or
/// `None` if `ty` is not such a union.
fn literal_union_members(ty: &Type, env: &TypeEnv) -> Option<Vec<Value>> {
    match ty {
        Type::Named(n) => env.resolve(n).and_then(|t| literal_union_members(t, env)),
        Type::Union(variants) => variants.iter().map(literal_value).collect(),
        _ => None,
    }
}

fn literal_value(ty: &Type) -> Option<Value> {
    match ty {
        Type::LitStr(s) => Some(Value::Str(s.clone())),
        Type::LitInt(n) => Some(Value::Int(n.clone())),
        Type::LitBool(b) => Some(Value::Bool(*b)),
        _ => None,
    }
}

/// Render a scalar into the text of an interpolation hole (§6.3). Interpolation
/// is value-level: only a scalar can land in a string — a list/map/bytes cannot,
/// so structure can never be smuggled into a string.
fn render_scalar(v: &Value) -> Result<String, Box<ValidationError>> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        Value::Int(n) => Ok(n.to_string()),
        Value::Decimal(d) => Ok(d.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(err(
            "",
            format!("{other:?}"),
            "a scalar (str/int/decimal/bool) to interpolate into a string",
        )),
    }
}

/// Whether a value tree contains an unresolved reference (so default validation
/// can skip a default that needs reduction first).
fn contains_ref(v: &Value) -> bool {
    match v {
        Value::Ref(_) | Value::Interp(_) | Value::Match { .. } => true,
        Value::List(xs) => xs.iter().any(contains_ref),
        Value::Map(m) => m.values().any(contains_ref),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_syntax::Type;

    fn types() -> TypeEnv {
        TypeEnv::build(&[], &[]).unwrap()
    }
    fn int(n: i64) -> Value {
        Value::Int(n.into())
    }

    #[test]
    fn ref_resolves_to_param_default() {
        let params = vec![Param {
            name: "n".into(),
            ty: Type::Int,
            default: Some(int(3)),
        }];
        let mut m = BTreeMap::new();
        m.insert("replicas".into(), Value::Ref("n".into()));
        let out = eval(&Value::Map(m), &params, &types()).unwrap();
        assert_eq!(
            out,
            Value::Map(BTreeMap::from([("replicas".into(), int(3))]))
        );
    }

    #[test]
    fn required_param_without_value_errors() {
        let params = vec![Param {
            name: "version".into(),
            ty: Type::Str,
            default: None,
        }];
        let e = eval(&Value::Map(BTreeMap::new()), &params, &types()).unwrap_err();
        assert!(e.path.contains("version"), "{e}");
    }

    #[test]
    fn unknown_reference_errors() {
        let mut m = BTreeMap::new();
        m.insert("x".into(), Value::Ref("nope".into()));
        let e = eval(&Value::Map(m), &[], &types()).unwrap_err();
        assert!(e.got.contains("nope"), "{e}");
    }

    #[test]
    fn sibling_binding_reference_resolves() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), int(7));
        m.insert("b".into(), Value::Ref("a".into()));
        let out = eval(&Value::Map(m), &[], &types()).unwrap();
        let Value::Map(o) = out else { panic!() };
        assert_eq!(o.get("b"), Some(&int(7)));
    }

    #[test]
    fn cyclic_reference_errors_not_overflows() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), Value::Ref("b".into()));
        m.insert("b".into(), Value::Ref("a".into()));
        assert!(eval(&Value::Map(m), &[], &types()).is_err());
    }

    #[test]
    fn bad_default_fails_at_the_param() {
        let params = vec![Param {
            name: "n".into(),
            ty: Type::Int,
            default: Some(Value::Str("oops".into())),
        }];
        let e = eval(&Value::Map(BTreeMap::new()), &params, &types()).unwrap_err();
        assert!(e.path.contains("params.n"), "{e}");
    }

    #[test]
    fn interpolation_renders_param_into_string() {
        let params = vec![Param {
            name: "v".into(),
            ty: Type::Str,
            default: Some(Value::Str("1.0".into())),
        }];
        let mut m = BTreeMap::new();
        m.insert(
            "image".into(),
            Value::Interp(vec![StrPart::Lit("api:".into()), StrPart::Ref("v".into())]),
        );
        let out = eval(&Value::Map(m), &params, &types()).unwrap();
        let Value::Map(o) = out else { panic!() };
        assert_eq!(o.get("image"), Some(&Value::Str("api:1.0".into())));
    }

    #[test]
    fn interpolating_a_non_scalar_errors() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), Value::List(vec![]));
        m.insert("s".into(), Value::Interp(vec![StrPart::Ref("a".into())]));
        assert!(eval(&Value::Map(m), &[], &types()).is_err());
    }

    fn matchv(scrutinee: &str, arms: Vec<(Option<Value>, Value)>) -> Value {
        Value::Match {
            scrutinee: Box::new(Value::Ref(scrutinee.into())),
            arms,
        }
    }

    #[test]
    fn match_selects_arm_union_exhaustive_without_wildcard() {
        let union = Type::Union(vec![
            Type::LitStr("dev".into()),
            Type::LitStr("staging".into()),
            Type::LitStr("prod".into()),
        ]);
        let params = vec![Param {
            name: "env".into(),
            ty: union,
            default: Some(Value::Str("prod".into())),
        }];
        let arms = vec![
            (Some(Value::Str("dev".into())), int(1)),
            (Some(Value::Str("staging".into())), int(2)),
            (Some(Value::Str("prod".into())), int(6)),
        ];
        let mut m = BTreeMap::new();
        m.insert("replicas".into(), matchv("env", arms));
        let out = eval(&Value::Map(m), &params, &types()).unwrap();
        let Value::Map(o) = out else { panic!() };
        assert_eq!(o.get("replicas"), Some(&int(6)));
    }

    #[test]
    fn wildcard_makes_match_exhaustive() {
        let mut m = BTreeMap::new();
        m.insert("x".into(), Value::Str("zzz".into()));
        m.insert(
            "y".into(),
            matchv(
                "x",
                vec![(Some(Value::Str("a".into())), int(1)), (None, int(0))],
            ),
        );
        let out = eval(&Value::Map(m), &[], &types()).unwrap();
        let Value::Map(o) = out else { panic!() };
        assert_eq!(o.get("y"), Some(&int(0))); // fell through to `_`
    }

    #[test]
    fn non_exhaustive_match_without_wildcard_errors() {
        // scrutinee is a sibling binding (no declared union) and there is no `_`.
        let mut m = BTreeMap::new();
        m.insert("x".into(), Value::Str("a".into()));
        m.insert(
            "y".into(),
            matchv("x", vec![(Some(Value::Str("a".into())), int(1))]),
        );
        assert!(eval(&Value::Map(m), &[], &types()).is_err());
    }

    #[test]
    fn match_missing_union_member_errors() {
        let union = Type::Union(vec![
            Type::LitStr("dev".into()),
            Type::LitStr("prod".into()),
        ]);
        let params = vec![Param {
            name: "env".into(),
            ty: union,
            default: Some(Value::Str("dev".into())),
        }];
        let mut m = BTreeMap::new();
        m.insert(
            "r".into(),
            matchv("env", vec![(Some(Value::Str("dev".into())), int(1))]), // missing "prod"
        );
        assert!(eval(&Value::Map(m), &params, &types()).is_err());
    }

    #[test]
    fn l2_body_passes_through_unchanged() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), int(1));
        let body = Value::Map(m);
        assert_eq!(eval(&body, &[], &types()).unwrap(), body);
    }
}
