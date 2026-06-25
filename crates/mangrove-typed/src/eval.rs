//! The L3 eval stage (§6): reduce expression markers in a composed body to plain
//! values against the param/binding environment. M4a covers `params` (§6.1) and
//! bare-name references (`Value::Ref`). It runs after compose and before
//! validate/resolve/hash, so the canonical form never contains an expression
//! (D35). An L0–L2 body has no markers and passes through unchanged.

use crate::env::TypeEnv;
use crate::validate::validate;
use mangrove_core::error::ValidationError;
use mangrove_core::{StrPart, Value};
use mangrove_syntax::Param;
use std::collections::BTreeMap;

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

    // Params first (they take precedence over sibling bindings, D36).
    for p in params {
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

    reduce(body, &scope, 0)
}

fn reduce(
    v: &Value,
    scope: &BTreeMap<String, Value>,
    depth: usize,
) -> Result<Value, Box<ValidationError>> {
    if depth >= MAX_DEPTH {
        return Err(err("", "a reference cycle", "an acyclic reference"));
    }
    match v {
        Value::Ref(name) => {
            let target = scope.get(name).ok_or_else(|| {
                err(
                    "",
                    format!("reference `{name}`"),
                    "a param or sibling binding",
                )
            })?;
            // Resolve transitively so a binding that is itself a reference works.
            reduce(target, scope, depth + 1)
        }
        Value::Interp(parts) => {
            let mut s = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(t) => s.push_str(t),
                    StrPart::Ref(name) => {
                        let target = scope.get(name).ok_or_else(|| {
                            err(
                                "",
                                format!("reference `{name}`"),
                                "a param or sibling binding",
                            )
                        })?;
                        let v = reduce(target, scope, depth + 1)?;
                        s.push_str(&render_scalar(&v)?);
                    }
                }
            }
            Ok(Value::Str(s))
        }
        Value::List(xs) => Ok(Value::List(
            xs.iter()
                .map(|x| reduce(x, scope, depth + 1))
                .collect::<Result<_, _>>()?,
        )),
        Value::Map(m) => {
            let mut out = BTreeMap::new();
            for (k, val) in m {
                out.insert(k.clone(), reduce(val, scope, depth + 1)?);
            }
            Ok(Value::Map(out))
        }
        other => Ok(other.clone()),
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
        Value::Ref(_) => true,
        Value::Interp(_) => true,
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

    #[test]
    fn l2_body_passes_through_unchanged() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), int(1));
        let body = Value::Map(m);
        assert_eq!(eval(&body, &[], &types()).unwrap(), body);
    }
}
