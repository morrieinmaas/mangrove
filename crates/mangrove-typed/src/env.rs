//! The type environment: named `type` definitions resolved for validation.
//! Building one rejects duplicate names, unknown references, and any recursive
//! (`Named`) cycle — recursion is forbidden by the totality axiom (§2.6).

use bigdecimal::BigDecimal;
use mangrove_syntax::{Annotation, Type, TypeDef, UnitDef};
use num_bigint::BigInt;
use std::collections::HashMap;

pub struct TypeEnv {
    types: HashMap<String, Type>,
    units: HashMap<String, Vec<(String, BigInt)>>,
    /// `@message(…)` per named type, surfaced in §12 errors (§4.9).
    messages: HashMap<String, String>,
}

impl TypeEnv {
    /// Build an environment from a document's `type` and `unit` definitions.
    /// Errors on a duplicate name, an unknown referenced type, or a `Named`
    /// cycle.
    pub fn build(typedefs: &[TypeDef], unitdefs: &[UnitDef]) -> Result<TypeEnv, String> {
        let mut types = HashMap::new();
        let mut messages = HashMap::new();
        for td in typedefs {
            if types.contains_key(&td.name) {
                return Err(format!("duplicate type definition: {}", td.name));
            }
            types.insert(td.name.clone(), td.ty.clone());
            if let Some(msg) = Annotation::find(&td.annotations, "message") {
                messages.insert(td.name.clone(), msg.to_string());
            }
        }
        let mut units: HashMap<String, Vec<(String, BigInt)>> = HashMap::new();
        for u in unitdefs {
            if units.contains_key(&u.name) || types.contains_key(&u.name) {
                return Err(format!("duplicate type/unit definition: {}", u.name));
            }
            units.insert(u.name.clone(), u.members.clone());
        }
        // Build the type-reference graph (a Named may also point at a unit type,
        // which is a valid leaf and never part of a cycle), reject unknown
        // references, then check for cycles iteratively — so an arbitrarily long
        // acyclic chain `A0 -> A1 -> … -> An` cannot overflow the call stack.
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for (name, ty) in &types {
            let mut refs = Vec::new();
            collect_refs(ty, &mut refs);
            for r in &refs {
                if !types.contains_key(r) && !units.contains_key(r) {
                    return Err(format!("unknown type: {r}"));
                }
            }
            let type_refs: Vec<String> =
                refs.into_iter().filter(|r| types.contains_key(r)).collect();
            adj.insert(name.clone(), type_refs);
        }
        detect_cycle(&adj)?;
        Ok(TypeEnv {
            types,
            units,
            messages,
        })
    }

    /// Look up a named type.
    pub fn resolve(&self, name: &str) -> Option<&Type> {
        self.types.get(name)
    }

    /// Whether `name` is a declared unit type.
    pub fn is_unit(&self, name: &str) -> bool {
        self.units.contains_key(name)
    }

    /// The `@message` declared on a named type, if any (§4.9).
    pub fn message(&self, name: &str) -> Option<&str> {
        self.messages.get(name).map(String::as_str)
    }

    /// Resolve a unit literal `mantissa<suffix>` against unit type `unit` to its
    /// base integer. Errors if the unit/suffix is unknown or the result is not
    /// an exact integer in the base unit (§4.5).
    pub fn resolve_unit(
        &self,
        unit: &str,
        mantissa: &BigDecimal,
        suffix: &str,
    ) -> Result<BigInt, String> {
        let members = self
            .units
            .get(unit)
            .ok_or_else(|| format!("unknown unit type: {unit}"))?;
        let base = members
            .iter()
            .find(|(n, _)| n == suffix)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| {
                let valid: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
                format!("unknown unit `{suffix}`; valid: {}", valid.join(", "))
            })?;
        let product = mantissa.clone() * BigDecimal::from(base);
        mangrove_core::exact_bigint(&product)
            .ok_or_else(|| format!("`{mantissa}{suffix}` is not an exact integer in the base unit"))
    }
}

/// Collect the names of every `Named` reference inside `ty`. Recurses over type
/// *structure* only (bounded by the parser's nesting cap), never across the
/// name-reference graph — that traversal is iterative (see `detect_cycle`).
fn collect_refs(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Named(n) => out.push(n.clone()),
        Type::Record { fields } => {
            for f in fields {
                collect_refs(&f.ty, out);
            }
        }
        Type::Map(inner) | Type::List(inner) => collect_refs(inner, out),
        Type::Brand { inner, .. } => collect_refs(inner, out),
        Type::Union(variants) => {
            for v in variants {
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Iterative three-colour DFS over the name-reference graph. Errors (naming a
/// node on the cycle) if one exists. The work stack lives on the heap, so chain
/// length is bounded only by memory, never by the call stack.
fn detect_cycle(adj: &HashMap<String, Vec<String>>) -> Result<(), String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        Gray,
        Black,
    }
    let mut color: HashMap<&str, Color> = HashMap::new();
    for start in adj.keys() {
        if color.contains_key(start.as_str()) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        color.insert(start.as_str(), Color::Gray);
        while let Some(&(node, idx)) = stack.last() {
            let children = &adj[node];
            if idx < children.len() {
                stack.last_mut().unwrap().1 += 1;
                let child = children[idx].as_str();
                match color.get(child) {
                    Some(Color::Gray) => {
                        return Err(format!("recursive type definition involving {child}"));
                    }
                    Some(Color::Black) => {}
                    None => {
                        color.insert(child, Color::Gray);
                        stack.push((child, 0));
                    }
                }
            } else {
                color.insert(node, Color::Black);
                stack.pop();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn bd(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }
    fn td(name: &str, ty: Type) -> TypeDef {
        TypeDef {
            name: name.into(),
            ty,
            annotations: vec![],
        }
    }
    fn bytes_unit() -> Vec<UnitDef> {
        vec![UnitDef {
            name: "Bytes".into(),
            members: vec![
                ("B".into(), 1.into()),
                ("Ki".into(), 1024.into()),
                ("Mi".into(), 1_048_576.into()),
            ],
        }]
    }

    #[test]
    fn resolves_named_types() {
        let env = TypeEnv::build(&[td("Port", Type::Int)], &[]).unwrap();
        assert_eq!(env.resolve("Port"), Some(&Type::Int));
        assert_eq!(env.resolve("Missing"), None);
    }

    #[test]
    fn duplicate_type_name_errors() {
        assert!(TypeEnv::build(&[td("A", Type::Int), td("A", Type::Str)], &[]).is_err());
    }

    #[test]
    fn direct_cycle_errors() {
        assert!(TypeEnv::build(&[td("A", Type::Named("A".into()))], &[]).is_err());
    }

    #[test]
    fn mutual_cycle_errors() {
        assert!(
            TypeEnv::build(
                &[
                    td("A", Type::Named("B".into())),
                    td("B", Type::Named("A".into())),
                ],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn cycle_through_container_errors() {
        assert!(
            TypeEnv::build(
                &[td("A", Type::List(Box::new(Type::Named("A".into()))))],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_referenced_type_errors() {
        assert!(TypeEnv::build(&[td("A", Type::Named("Nope".into()))], &[]).is_err());
    }

    #[test]
    fn named_may_reference_a_unit_type() {
        // A = Named("Bytes") where Bytes is a unit — valid, not "unknown type".
        let env = TypeEnv::build(&[td("A", Type::Named("Bytes".into()))], &bytes_unit());
        assert!(env.is_ok());
        assert!(env.unwrap().is_unit("Bytes"));
    }

    #[test]
    fn non_recursive_nested_is_fine() {
        let env = TypeEnv::build(
            &[
                td(
                    "A",
                    Type::Record {
                        fields: vec![mangrove_syntax::FieldDef {
                            name: "b".into(),
                            optional: false,
                            ty: Type::Named("B".into()),
                            default: None,
                            annotations: vec![],
                        }],
                    },
                ),
                td("B", Type::Int),
            ],
            &[],
        );
        assert!(env.is_ok());
    }

    #[test]
    fn long_acyclic_chain_does_not_overflow() {
        let n = 50_000;
        let mut defs: Vec<TypeDef> = (0..n)
            .map(|i| td(&format!("A{i}"), Type::Named(format!("A{}", i + 1))))
            .collect();
        defs.push(td(&format!("A{n}"), Type::Int));
        assert!(TypeEnv::build(&defs, &[]).is_ok());
    }

    #[test]
    fn message_annotation_is_stored() {
        let env = TypeEnv::build(
            &[TypeDef {
                name: "Port".into(),
                ty: Type::Int,
                annotations: vec![Annotation {
                    name: "message".into(),
                    arg: Some("bad port".into()),
                }],
            }],
            &[],
        )
        .unwrap();
        assert_eq!(env.message("Port"), Some("bad port"));
    }

    #[test]
    fn resolve_unit_literal_to_base_int() {
        let env = TypeEnv::build(&[], &bytes_unit()).unwrap();
        assert_eq!(
            env.resolve_unit("Bytes", &512.into(), "Mi").unwrap(),
            536_870_912.into()
        );
    }

    #[test]
    fn unknown_suffix_errors_with_valid_list() {
        let env = TypeEnv::build(&[], &bytes_unit()).unwrap();
        let e = env.resolve_unit("Bytes", &256.into(), "MB").unwrap_err();
        assert!(e.contains("MB") && e.contains("Mi"));
    }

    #[test]
    fn fractional_must_be_exact() {
        let units = vec![UnitDef {
            name: "Sats".into(),
            members: vec![("sat".into(), 1.into()), ("btc".into(), 100_000_000.into())],
        }];
        let env = TypeEnv::build(&[], &units).unwrap();
        assert_eq!(
            env.resolve_unit("Sats", &bd("0.5"), "btc").unwrap(),
            50_000_000.into()
        );
        assert!(env.resolve_unit("Sats", &bd("0.5"), "sat").is_err());
    }
}
