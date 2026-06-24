//! The type environment: named `type` definitions resolved for validation.
//! Building one rejects duplicate names, unknown references, and any recursive
//! (`Named`) cycle — recursion is forbidden by the totality axiom (§2.6).

use mangrove_syntax::Type;
use std::collections::HashMap;

pub struct TypeEnv {
    types: HashMap<String, Type>,
}

impl TypeEnv {
    /// Build an environment from a document's `type` definitions. Errors on a
    /// duplicate type name, an unknown referenced type, or a `Named` cycle.
    pub fn build(typedefs: &[(String, Type)]) -> Result<TypeEnv, String> {
        let mut types = HashMap::new();
        for (name, ty) in typedefs {
            if types.contains_key(name) {
                return Err(format!("duplicate type definition: {name}"));
            }
            types.insert(name.clone(), ty.clone());
        }
        // Build the name-reference graph (rejecting unknown references), then
        // check it for cycles iteratively — so an arbitrarily long acyclic
        // chain `A0 -> A1 -> … -> An` cannot overflow the call stack.
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for (name, ty) in &types {
            let mut refs = Vec::new();
            collect_refs(ty, &mut refs);
            for r in &refs {
                if !types.contains_key(r) {
                    return Err(format!("unknown type: {r}"));
                }
            }
            adj.insert(name.clone(), refs);
        }
        detect_cycle(&adj)?;
        Ok(TypeEnv { types })
    }

    /// Look up a named type.
    pub fn resolve(&self, name: &str) -> Option<&Type> {
        self.types.get(name)
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

    #[test]
    fn resolves_named_types() {
        let env = TypeEnv::build(&[("Port".into(), Type::Int)]).unwrap();
        assert_eq!(env.resolve("Port"), Some(&Type::Int));
        assert_eq!(env.resolve("Missing"), None);
    }

    #[test]
    fn duplicate_type_name_errors() {
        assert!(TypeEnv::build(&[("A".into(), Type::Int), ("A".into(), Type::Str)]).is_err());
    }

    #[test]
    fn direct_cycle_errors() {
        assert!(TypeEnv::build(&[("A".into(), Type::Named("A".into()))]).is_err());
    }

    #[test]
    fn mutual_cycle_errors() {
        assert!(
            TypeEnv::build(&[
                ("A".into(), Type::Named("B".into())),
                ("B".into(), Type::Named("A".into())),
            ])
            .is_err()
        );
    }

    #[test]
    fn cycle_through_container_errors() {
        // A = [ A ] — a cycle through a list child.
        assert!(
            TypeEnv::build(&[("A".into(), Type::List(Box::new(Type::Named("A".into()))))]).is_err()
        );
    }

    #[test]
    fn unknown_referenced_type_errors() {
        assert!(TypeEnv::build(&[("A".into(), Type::Named("Nope".into()))]).is_err());
    }

    #[test]
    fn non_recursive_nested_is_fine() {
        let env = TypeEnv::build(&[
            (
                "A".into(),
                Type::Record {
                    fields: vec![mangrove_syntax::FieldDef {
                        name: "b".into(),
                        optional: false,
                        ty: Type::Named("B".into()),
                    }],
                },
            ),
            ("B".into(), Type::Int),
        ]);
        assert!(env.is_ok());
    }

    #[test]
    fn long_acyclic_chain_does_not_overflow() {
        // Was: SIGABRT via recursive cycle-check. Now iterative — builds fine.
        let n = 50_000;
        let mut defs: Vec<(String, Type)> = (0..n)
            .map(|i| (format!("A{i}"), Type::Named(format!("A{}", i + 1))))
            .collect();
        defs.push((format!("A{n}"), Type::Int));
        assert!(TypeEnv::build(&defs).is_ok());
    }
}
