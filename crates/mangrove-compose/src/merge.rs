//! The composition merge engine (§5.3, §5.4): one rule — later wins,
//! records/maps deep-merge, everything else replaces; `unset` removes a key.

use mangrove_core::Value;

/// Bound on merge recursion — guards against stack overflow on pathologically
/// deep inputs (cf. the parser's MAX_DEPTH; parsed documents never exceed it,
/// so this only bites hand-built API inputs). Real configs are far shallower.
const MAX_DEPTH: usize = 128;

/// Merge `over` onto `base` (D20): if both are maps, deep-merge key-by-key
/// (a key whose `over` value is `Unset` is removed); otherwise `over` replaces.
/// The result never contains `Unset` (orphan `unset`s — with no inherited key
/// to remove — collapse to absence, §5.4).
pub fn merge(base: Value, over: Value) -> Value {
    merge_d(base, over, 0)
}

fn merge_d(base: Value, over: Value, depth: usize) -> Value {
    match (base, over) {
        (Value::Map(b), Value::Map(o)) if depth < MAX_DEPTH => {
            let mut out = b;
            for (k, v) in o {
                if matches!(v, Value::Unset) {
                    out.remove(&k);
                } else if let Some(existing) = out.remove(&k) {
                    out.insert(k, merge_d(existing, v, depth + 1));
                } else {
                    out.insert(k, strip_unset_d(v, depth + 1));
                }
            }
            Value::Map(out)
        }
        (_, over) => strip_unset_d(over, depth),
    }
}

/// Remove any `Unset` entries from a value with no inherited counterpart to
/// remove — they simply mean "absent" (§5.4). Depth-bounded like `merge_d`.
fn strip_unset_d(v: Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return v;
    }
    match v {
        Value::Map(m) => Value::Map(
            m.into_iter()
                .filter(|(_, val)| !matches!(val, Value::Unset))
                .map(|(k, val)| (k, strip_unset_d(val, depth + 1)))
                .collect(),
        ),
        Value::List(xs) => Value::List(
            xs.into_iter()
                .map(|x| strip_unset_d(x, depth + 1))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::merge;
    use mangrove_core::Value;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, Value)]) -> Value {
        Value::Map(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }
    fn i(n: i64) -> Value {
        Value::Int(n.into())
    }

    #[test]
    fn deep_merge_and_scalar_override() {
        let base = map(&[("a", i(1)), ("b", i(2)), ("nested", map(&[("x", i(1))]))]);
        let over = map(&[("b", i(9)), ("nested", map(&[("y", i(2))]))]);
        let merged = merge(base, over);
        assert_eq!(
            merged,
            map(&[
                ("a", i(1)),
                ("b", i(9)),                                  // overridden
                ("nested", map(&[("x", i(1)), ("y", i(2))])), // deep-merged
            ])
        );
    }

    #[test]
    fn list_replaces_not_merges() {
        let base = map(&[("xs", Value::List(vec![i(1), i(2)]))]);
        let over = map(&[("xs", Value::List(vec![i(3)]))]);
        assert_eq!(merge(base, over), map(&[("xs", Value::List(vec![i(3)]))]));
    }

    #[test]
    fn unset_removes_inherited_key() {
        let base = map(&[("a", i(1)), ("b", i(2))]);
        let over = map(&[("b", Value::Unset)]);
        assert_eq!(merge(base, over), map(&[("a", i(1))]));
    }

    #[test]
    fn orphan_unset_collapses_to_absence() {
        let base = map(&[("a", i(1))]);
        let over = map(&[("ghost", Value::Unset), ("c", i(3))]);
        // `ghost: unset` had nothing to remove → absent; `c` added.
        assert_eq!(merge(base, over), map(&[("a", i(1)), ("c", i(3))]));
    }

    #[test]
    fn deep_inputs_do_not_overflow_merge_recursion() {
        // Nesting past MAX_DEPTH must make merge *return* (cap), not recurse
        // unboundedly. Depth 300 > MAX_DEPTH(128) yet shallow enough that the
        // value's own recursive Drop is safe (parsed inputs are ≤128 anyway).
        let deep = |seed: i64| {
            let mut v = Value::Int(seed.into());
            for _ in 0..300 {
                let mut m = BTreeMap::new();
                m.insert("n".to_string(), v);
                v = Value::Map(m);
            }
            v
        };
        let _ = merge(deep(1), deep(2)); // bounded by MAX_DEPTH; returns
    }
}
