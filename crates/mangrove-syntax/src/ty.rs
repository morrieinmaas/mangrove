//! The L1 type AST. Produced by the type-grammar parser (`parser.rs`),
//! consumed by `mangrove-typed` for resolution and validation.

use bigdecimal::BigDecimal;
use num_bigint::BigInt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    // primitives (§3.2)
    Int,
    Decimal,
    Str,
    Bool,
    Bytes,

    // refinements (§4.3) — interval bounds (inclusive) and regex
    IntRange {
        min: Option<BigInt>,
        max: Option<BigInt>,
    },
    DecRange {
        min: Option<BigDecimal>,
        max: Option<BigDecimal>,
    },
    StrRegex(String),

    // literals (for unions / enums)
    LitStr(String),
    LitInt(BigInt),
    LitBool(bool),

    // composites
    Record {
        fields: Vec<FieldDef>,
    },
    Map(Box<Type>),
    List(Box<Type>),
    Union(Vec<Type>),

    // reference to a `type X = …`, resolved against a TypeEnv
    Named(String),

    // nominal newtype (§4.6): distinct identity over a structural `inner`.
    // `name` is filled from the enclosing `type X = brand …` binding.
    Brand {
        name: String,
        inner: Box<Type>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub optional: bool,
    pub ty: Type,
}

#[cfg(test)]
mod tests {
    use crate::parse_type;
    use crate::ty::*;

    fn pt(s: &str) -> Type {
        parse_type(s).unwrap()
    }

    #[test]
    fn primitive() {
        assert_eq!(pt("int"), Type::Int);
        assert_eq!(pt("str"), Type::Str);
    }

    #[test]
    fn int_range() {
        assert_eq!(
            pt("int & >= 1 & <= 10"),
            Type::IntRange {
                min: Some(1.into()),
                max: Some(10.into()),
            }
        );
    }

    #[test]
    fn strict_int_bounds_normalize_to_inclusive() {
        // int & > 0 & < 10  ==  >= 1 & <= 9
        assert_eq!(
            pt("int & > 0 & < 10"),
            Type::IntRange {
                min: Some(1.into()),
                max: Some(9.into()),
            }
        );
    }

    #[test]
    fn str_regex() {
        assert_eq!(pt("str & =~ \"^a+$\""), Type::StrRegex("^a+$".into()));
    }

    #[test]
    fn union_of_literals() {
        assert_eq!(
            pt("\"dev\" | \"prod\""),
            Type::Union(vec![
                Type::LitStr("dev".into()),
                Type::LitStr("prod".into())
            ])
        );
    }

    #[test]
    fn record_with_optional() {
        let Type::Record { fields } = pt("{ host: str, port: int, tls?: bool }") else {
            panic!()
        };
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().find(|f| f.name == "tls").unwrap().optional);
        assert!(!fields.iter().find(|f| f.name == "host").unwrap().optional);
    }

    #[test]
    fn empty_record() {
        assert_eq!(pt("{}"), Type::Record { fields: vec![] });
    }

    #[test]
    fn map_and_list() {
        assert_eq!(pt("{ [str]: int }"), Type::Map(Box::new(Type::Int)));
        assert_eq!(pt("[ str ]"), Type::List(Box::new(Type::Str)));
    }

    #[test]
    fn named() {
        assert_eq!(pt("Port"), Type::Named("Port".into()));
    }

    #[test]
    fn refinement_atom_mismatch_errors() {
        // F2 / D10: bounds only on int/decimal, regex only on str
        assert!(parse_type("str & >= 1").is_err());
        assert!(parse_type("int & =~ \"re\"").is_err());
        assert!(parse_type("bool & >= 1").is_err());
    }

    #[test]
    fn deeply_nested_type_errors_instead_of_overflowing() {
        // Was: SIGABRT stack overflow. Now a clean error well before the limit.
        let list = format!("{}int{}", "[".repeat(5000), "]".repeat(5000));
        assert!(parse_type(&list).is_err());
        let record = format!("{}int{}", "{ a: ".repeat(5000), " }".repeat(5000));
        assert!(parse_type(&record).is_err());
        // A reasonable nesting depth still parses fine.
        let ok = format!("{}int{}", "[".repeat(50), "]".repeat(50));
        assert!(parse_type(&ok).is_ok());
    }
}
