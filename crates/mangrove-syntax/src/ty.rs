//! The L1 type AST. Produced by the type-grammar parser (`parser.rs`),
//! consumed by `mangrove-typed` for resolution and validation.

use bigdecimal::BigDecimal;
use mangrove_core::Value;
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
        /// Cross-field predicates (§4.7), evaluated against concrete values.
        requires: Vec<Require>,
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
    /// Default value (`field: T | *value`), materialized into the canonical
    /// form when the field is absent (§4.7, §7 step 3).
    pub default: Option<Value>,
    /// Metadata annotations (`@doc`/`@message`/`@deprecated`, §4.9). Never part
    /// of the data hash.
    pub annotations: Vec<Annotation>,
}

/// A metadata annotation: `@name(arg)` (§4.9). `arg` is the single string
/// argument, when present.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub name: String,
    pub arg: Option<String>,
}

/// A cross-field constraint (§4.7): a total predicate over the record's fields,
/// with an optional `@message`.
#[derive(Debug, Clone, PartialEq)]
pub struct Require {
    pub pred: Pred,
    pub message: Option<String>,
}

/// The `require` predicate sublanguage (total, decidable; §4.7).
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    Or(Box<Pred>, Box<Pred>),
    And(Box<Pred>, Box<Pred>),
    Not(Box<Pred>),
    Compare {
        op: CmpOp,
        lhs: Operand,
        rhs: Operand,
    },
    /// A bare operand used as a boolean (e.g. a `bool` field, or `(pred)`).
    Truthy(Operand),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A dotted field path into the record in scope, e.g. `cpu.limit`.
    Path(Vec<String>),
    Int(BigInt),
    Decimal(BigDecimal),
    Str(String),
    Bool(bool),
    /// `len(path)` — element/char count of a list/map/string.
    Len(Vec<String>),
    /// A parenthesized sub-predicate used as a value.
    Pred(Box<Pred>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Annotation {
    /// The `@<name>` argument of the first matching annotation, if any.
    pub fn find<'a>(anns: &'a [Annotation], name: &str) -> Option<&'a str> {
        anns.iter()
            .find(|a| a.name == name)
            .and_then(|a| a.arg.as_deref())
    }
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
        let Type::Record { fields, .. } = pt("{ host: str, port: int, tls?: bool }") else {
            panic!()
        };
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().find(|f| f.name == "tls").unwrap().optional);
        assert!(!fields.iter().find(|f| f.name == "host").unwrap().optional);
    }

    #[test]
    fn empty_record() {
        assert_eq!(
            pt("{}"),
            Type::Record {
                fields: vec![],
                requires: vec![]
            }
        );
    }

    #[test]
    fn field_defaults_parse() {
        use mangrove_core::Value;
        let Type::Record { fields, .. } = pt("{ ns: str | *\"d\", n: int | *1, f?: bool }") else {
            panic!()
        };
        let ns = fields.iter().find(|f| f.name == "ns").unwrap();
        assert_eq!(ns.default, Some(Value::Str("d".into())));
        let n = fields.iter().find(|f| f.name == "n").unwrap();
        assert_eq!(n.default, Some(Value::Int(1.into())));
        let f = fields.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(f.default, None);
        assert!(f.optional);
    }

    #[test]
    fn real_union_field_not_confused_with_default() {
        let Type::Record { fields, .. } = pt("{ a: str | int }") else {
            panic!()
        };
        assert!(matches!(fields[0].ty, Type::Union(_)));
        assert_eq!(fields[0].default, None);
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
