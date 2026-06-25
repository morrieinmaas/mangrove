//! Evaluate `require` predicates (§4.7) against a concrete record value. Total
//! and decidable: comparisons, `&&`/`||`/`!`, `len`, field paths. No user
//! functions, no recursion over the data.

use bigdecimal::BigDecimal;
use mangrove_core::Value;
use mangrove_syntax::{CmpOp, Operand, Pred};
use num_bigint::BigInt;
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Evaluate `pred` against the record `rec`. `Err` on an ill-formed comparison
/// (e.g. ordering across kinds, or a path to a missing/non-record field) — the
/// caller treats both `Ok(false)` and `Err` as a failed require.
pub fn eval_pred(pred: &Pred, rec: &BTreeMap<String, Value>) -> Result<bool, String> {
    match pred {
        Pred::Or(a, b) => Ok(eval_pred(a, rec)? || eval_pred(b, rec)?),
        Pred::And(a, b) => Ok(eval_pred(a, rec)? && eval_pred(b, rec)?),
        Pred::Not(p) => Ok(!eval_pred(p, rec)?),
        Pred::Truthy(op) => as_bool(&eval_operand(op, rec)?),
        Pred::Compare { op, lhs, rhs } => {
            compare(*op, &eval_operand(lhs, rec)?, &eval_operand(rhs, rec)?)
        }
    }
}

fn eval_operand(op: &Operand, rec: &BTreeMap<String, Value>) -> Result<Value, String> {
    match op {
        Operand::Path(segs) => resolve_path(segs, rec),
        Operand::Int(n) => Ok(Value::Int(n.clone())),
        Operand::Decimal(d) => Ok(Value::Decimal(d.clone())),
        Operand::Str(s) => Ok(Value::Str(s.clone())),
        Operand::Bool(b) => Ok(Value::Bool(*b)),
        Operand::Len(segs) => {
            let n = match resolve_path(segs, rec)? {
                Value::List(xs) => xs.len(),
                Value::Map(m) => m.len(),
                Value::Str(s) => s.chars().count(),
                _ => return Err("len() expects a list, map, or string".into()),
            };
            Ok(Value::Int(BigInt::from(n)))
        }
        Operand::Pred(p) => Ok(Value::Bool(eval_pred(p, rec)?)),
    }
}

fn resolve_path(segs: &[String], rec: &BTreeMap<String, Value>) -> Result<Value, String> {
    let mut cur = rec
        .get(&segs[0])
        .ok_or_else(|| format!("unknown field `{}`", segs[0]))?;
    for s in &segs[1..] {
        match cur {
            Value::Map(m) => {
                cur = m.get(s).ok_or_else(|| format!("unknown field `{s}`"))?;
            }
            _ => return Err(format!("`{s}` is not a field of a record")),
        }
    }
    Ok(cur.clone())
}

fn as_num(v: &Value) -> Option<BigDecimal> {
    match v {
        Value::Int(n) => Some(BigDecimal::from(n.clone())),
        Value::Decimal(d) => Some(d.clone()),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err("expected a boolean".into()),
    }
}

fn values_eq(l: &Value, r: &Value) -> bool {
    if let (Some(a), Some(b)) = (as_num(l), as_num(r)) {
        return a == b;
    }
    match (l, r) {
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

fn order(l: &Value, r: &Value) -> Result<Ordering, String> {
    if let (Some(a), Some(b)) = (as_num(l), as_num(r)) {
        return Ok(a.cmp(&b));
    }
    match (l, r) {
        (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
        _ => Err("cannot order these operands".into()),
    }
}

fn compare(op: CmpOp, l: &Value, r: &Value) -> Result<bool, String> {
    match op {
        CmpOp::Eq => Ok(values_eq(l, r)),
        CmpOp::Ne => Ok(!values_eq(l, r)),
        CmpOp::Lt => Ok(order(l, r)?.is_lt()),
        CmpOp::Le => Ok(order(l, r)?.is_le()),
        CmpOp::Gt => Ok(order(l, r)?.is_gt()),
        CmpOp::Ge => Ok(order(l, r)?.is_ge()),
    }
}

#[cfg(test)]
mod tests {
    use super::eval_pred;
    use mangrove_core::Value;
    use mangrove_syntax::parse_type;
    use std::collections::BTreeMap;

    /// Build a record value and pull the single require's predicate from `ty_src`.
    fn check(ty_src: &str, rec: &[(&str, Value)]) -> Result<bool, String> {
        let ty = parse_type(ty_src).unwrap();
        let mangrove_syntax::Type::Record { requires, .. } = ty else {
            panic!()
        };
        let map: BTreeMap<String, Value> = rec
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        eval_pred(&requires[0].pred, &map)
    }

    #[test]
    fn comparison_and_bool_ops() {
        let t = "{ a: int, b: int, require: a <= b }";
        assert_eq!(
            check(
                t,
                &[("a", Value::Int(1.into())), ("b", Value::Int(2.into()))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                t,
                &[("a", Value::Int(5.into())), ("b", Value::Int(2.into()))]
            ),
            Ok(false)
        );
    }

    #[test]
    fn or_and_len() {
        let t = "{ tls: bool, certs: [str], require: tls == false || len(certs) >= 1 }";
        // tls true, no certs → false (must have a cert)
        assert_eq!(
            check(
                t,
                &[("tls", Value::Bool(true)), ("certs", Value::List(vec![]))]
            ),
            Ok(false)
        );
        // tls false → ok regardless of certs
        assert_eq!(
            check(
                t,
                &[("tls", Value::Bool(false)), ("certs", Value::List(vec![]))]
            ),
            Ok(true)
        );
    }

    #[test]
    fn cross_kind_order_errors_not_panics() {
        let t = "{ a: str, b: int, require: a < b }";
        assert!(
            check(
                t,
                &[("a", Value::Str("x".into())), ("b", Value::Int(1.into()))]
            )
            .is_err()
        );
    }

    #[test]
    fn missing_field_errors() {
        let t = "{ a: int, require: a <= b }"; // b not a field
        assert!(check(t, &[("a", Value::Int(1.into()))]).is_err());
    }

    fn i(n: i64) -> Value {
        Value::Int(n.into())
    }

    #[test]
    fn every_comparison_operator() {
        let cmp = |op: &str, a: i64, b: i64| {
            let t = format!("{{ a: int, b: int, require: a {op} b }}");
            check(&t, &[("a", i(a)), ("b", i(b))])
        };
        assert_eq!(cmp("==", 1, 1), Ok(true));
        assert_eq!(cmp("==", 1, 2), Ok(false));
        assert_eq!(cmp("!=", 1, 2), Ok(true));
        assert_eq!(cmp("!=", 2, 2), Ok(false));
        assert_eq!(cmp("<", 1, 2), Ok(true));
        assert_eq!(cmp("<=", 2, 2), Ok(true));
        assert_eq!(cmp(">", 3, 2), Ok(true));
        assert_eq!(cmp(">=", 2, 2), Ok(true));
    }

    #[test]
    fn and_not_truthy() {
        assert_eq!(
            check(
                "{ a: int, b: int, c: int, require: a < b && b < c }",
                &[("a", i(1)), ("b", i(2)), ("c", i(3))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                "{ a: int, b: int, require: !(a == b) }",
                &[("a", i(1)), ("b", i(2))]
            ),
            Ok(true)
        );
        // a bare boolean field is a truthy predicate
        assert_eq!(
            check(
                "{ flag: bool, require: flag }",
                &[("flag", Value::Bool(true))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                "{ flag: bool, require: flag }",
                &[("flag", Value::Bool(false))]
            ),
            Ok(false)
        );
        // truthy on a non-bool → error, not panic
        assert!(check("{ a: int, require: a }", &[("a", i(1))]).is_err());
    }

    #[test]
    fn string_bool_and_decimal_operands() {
        assert_eq!(
            check(
                "{ s: str, require: s == \"x\" }",
                &[("s", Value::Str("x".into()))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                "{ b: bool, require: b == true }",
                &[("b", Value::Bool(true))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                "{ r: decimal, require: r >= 0.5 }",
                &[("r", Value::Decimal("0.5".parse().unwrap()))]
            ),
            Ok(true)
        );
        // cross-kind equality is false, not an error
        assert_eq!(
            check(
                "{ a: int, s: str, require: a == s }",
                &[("a", i(1)), ("s", Value::Str("1".into()))]
            ),
            Ok(false)
        );
    }

    #[test]
    fn len_on_map_string_and_non_container() {
        let mut m = BTreeMap::new();
        m.insert("k".to_string(), i(1));
        assert_eq!(
            check(
                "{ m: { k: int }, require: len(m) >= 1 }",
                &[("m", Value::Map(m))]
            ),
            Ok(true)
        );
        assert_eq!(
            check(
                "{ s: str, require: len(s) == 3 }",
                &[("s", Value::Str("abc".into()))]
            ),
            Ok(true)
        );
        assert!(check("{ a: int, require: len(a) >= 1 }", &[("a", i(1))]).is_err());
    }

    #[test]
    fn nested_field_path() {
        let mut inner = BTreeMap::new();
        inner.insert("x".to_string(), i(5));
        assert_eq!(
            check(
                "{ inner: { x: int }, require: inner.x >= 1 }",
                &[("inner", Value::Map(inner))]
            ),
            Ok(true)
        );
        // a path that descends into a non-record errors
        assert!(check("{ a: int, require: a.x >= 1 }", &[("a", i(1))]).is_err());
    }
}
