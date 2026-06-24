//! The type environment: named `type` definitions resolved for validation.
//! Building one rejects duplicate names and any recursive (`Named`) cycle —
//! recursion is forbidden by the totality axiom (§2.6).

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
        let env = TypeEnv { types };
        for name in env.types.keys() {
            let mut stack = Vec::new();
            env.check_acyclic(name, &mut stack)?;
        }
        Ok(env)
    }

    /// Look up a named type.
    pub fn resolve(&self, name: &str) -> Option<&Type> {
        self.types.get(name)
    }

    fn check_acyclic(&self, name: &str, stack: &mut Vec<String>) -> Result<(), String> {
        if stack.iter().any(|n| n == name) {
            stack.push(name.to_string());
            return Err(format!("recursive type definition: {}", stack.join(" -> ")));
        }
        let Some(ty) = self.types.get(name) else {
            return Err(format!("unknown type: {name}"));
        };
        stack.push(name.to_string());
        self.walk(ty, stack)?;
        stack.pop();
        Ok(())
    }

    fn walk(&self, ty: &Type, stack: &mut Vec<String>) -> Result<(), String> {
        match ty {
            Type::Named(n) => self.check_acyclic(n, stack),
            Type::Record { fields } => {
                for f in fields {
                    self.walk(&f.ty, stack)?;
                }
                Ok(())
            }
            Type::Map(inner) | Type::List(inner) => self.walk(inner, stack),
            Type::Union(variants) => {
                for v in variants {
                    self.walk(v, stack)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
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
    fn unknown_referenced_type_errors() {
        assert!(TypeEnv::build(&[("A".into(), Type::Named("Nope".into()))]).is_err());
    }

    #[test]
    fn non_recursive_nested_is_fine() {
        // A references B, B is a leaf — no cycle.
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
}
