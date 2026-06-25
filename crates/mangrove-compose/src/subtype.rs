//! The subtype relation `New <: Old` for subtype redefinition (§5.5, D23):
//! structural, covariant, depth-recursive. Used to admit a `schema Base & {…}`
//! narrowing only when every value valid under New was valid under Old.
//!
//! Decidable cases only: interval containment (int/decimal), enum/literal
//! subset, structural records (field-wise, required-ness monotone), covariant
//! lists/maps, union drop/narrow. Regex containment is **not** computed —
//! a `=~` narrowing is admitted only if byte-identical (stated limit). `require`
//! predicates are ignored here (re-validated against values, never implied).

use mangrove_syntax::Type;
use mangrove_typed::TypeEnv;

const MAX_DEPTH: usize = 128;

/// Build the effective schema for `schema Base & { narrow }` (§5.5): replace the
/// listed fields of `base` with the narrowing's field types, then verify the
/// result is a subtype of `base`. Errors if the narrowing adds an unknown field
/// or is not a valid narrowing.
pub fn narrowed_schema(base: &Type, narrow: &Type, env: &TypeEnv) -> Result<Type, String> {
    // Resolve `base` to a record.
    let base = resolve_record(base, env)?;
    let Type::Record {
        fields: base_fields,
        requires,
    } = base
    else {
        unreachable!()
    };
    let Type::Record {
        fields: nfields, ..
    } = narrow
    else {
        return Err("a subtype redefinition must be a record `& { … }`".into());
    };
    let mut new_fields = base_fields.clone();
    for nf in nfields {
        match new_fields.iter_mut().find(|f| f.name == nf.name) {
            Some(f) => f.ty = nf.ty.clone(),
            None => {
                return Err(format!(
                    "subtype redefinition adds unknown field `{}` (narrowing only)",
                    nf.name
                ));
            }
        }
    }
    let new_type = Type::Record {
        fields: new_fields,
        requires: requires.clone(),
    };
    let base_type = Type::Record {
        fields: base_fields.clone(),
        requires,
    };
    is_subtype(&new_type, &base_type, env)?;
    Ok(new_type)
}

/// Resolve a (possibly `Named`) type to an owned `Record`, or error.
fn resolve_record(ty: &Type, env: &TypeEnv) -> Result<Type, String> {
    match ty {
        Type::Record { .. } => Ok(ty.clone()),
        Type::Named(n) => {
            let t = env
                .resolve(n)
                .ok_or_else(|| format!("unknown base type `{n}`"))?;
            resolve_record(t, env)
        }
        _ => Err("base of a subtype redefinition must be a record".into()),
    }
}

/// `Ok(())` iff `new <: old`. Errors describe the first violation.
pub fn is_subtype(new: &Type, old: &Type, env: &TypeEnv) -> Result<(), String> {
    sub(new, old, env, 0)
}

fn sub(new: &Type, old: &Type, env: &TypeEnv, depth: usize) -> Result<(), String> {
    if depth >= MAX_DEPTH {
        return Err("type nesting too deep for subtype check".into());
    }
    // Resolve Named on either side (a unit name resolves to its int base kind).
    if let Type::Named(n) = new {
        return match env.resolve(n) {
            Some(t) => sub(t, old, env, depth + 1),
            None if env.is_unit(n) => sub(&Type::Int, old, env, depth + 1),
            None => Err(format!("unknown type `{n}`")),
        };
    }
    if let Type::Named(o) = old {
        return match env.resolve(o) {
            Some(t) => sub(new, t, env, depth + 1),
            None if env.is_unit(o) => sub(new, &Type::Int, env, depth + 1),
            None => Err(format!("unknown type `{o}`")),
        };
    }

    // Unions: New <: Old(union) iff each New variant is <: some Old variant.
    if let Type::Union(olds) = old {
        return match new {
            Type::Union(news) => {
                for nv in news {
                    if !olds.iter().any(|ov| sub(nv, ov, env, depth + 1).is_ok()) {
                        return Err("union variant not covered by the wider union".into());
                    }
                }
                Ok(())
            }
            _ => {
                if olds.iter().any(|ov| sub(new, ov, env, depth + 1).is_ok()) {
                    Ok(())
                } else {
                    Err("value type not a member of the wider union".into())
                }
            }
        };
    }
    if let Type::Union(news) = new {
        // every variant must be <: old
        for nv in news {
            sub(nv, old, env, depth + 1)?;
        }
        return Ok(());
    }

    match (new, old) {
        // exact primitive matches
        (Type::Int, Type::Int)
        | (Type::Decimal, Type::Decimal)
        | (Type::Str, Type::Str)
        | (Type::Bool, Type::Bool)
        | (Type::Bytes, Type::Bytes) => Ok(()),

        // int refinement <: int / int-range  (interval containment).
        // (Int, Int) is handled by the exact-match arm above.
        (Type::IntRange { .. }, Type::Int) => Ok(()),
        (
            n,
            Type::IntRange {
                min: omin,
                max: omax,
            },
        ) => {
            let (nmin, nmax) = match n {
                Type::IntRange { min, max } => (min.clone(), max.clone()),
                Type::Int => (None, None),
                Type::LitInt(v) => (Some(v.clone()), Some(v.clone())),
                _ => return Err("not an int subtype".into()),
            };
            // New's interval must be contained in Old's: New.min >= Old.min, New.max <= Old.max
            if let Some(o) = omin {
                match &nmin {
                    Some(nv) if nv >= o => {}
                    _ => return Err(format!("lower bound not >= {o}")),
                }
            }
            if let Some(o) = omax {
                match &nmax {
                    Some(nv) if nv <= o => {}
                    _ => return Err(format!("upper bound not <= {o}")),
                }
            }
            Ok(())
        }
        (Type::DecRange { .. }, Type::Decimal) => Ok(()),
        (
            n,
            Type::DecRange {
                min: omin,
                max: omax,
            },
        ) => {
            let (nmin, nmax) = match n {
                Type::DecRange { min, max } => (min.clone(), max.clone()),
                Type::Decimal => (None, None),
                _ => return Err("not a decimal subtype".into()),
            };
            if let Some(o) = omin
                && !matches!(&nmin, Some(nv) if nv >= o)
            {
                return Err(format!("lower bound not >= {o}"));
            }
            if let Some(o) = omax
                && !matches!(&nmax, Some(nv) if nv <= o)
            {
                return Err(format!("upper bound not <= {o}"));
            }
            Ok(())
        }

        // strings & regex (containment deferred — identical only).
        // (Str, Str) is handled by the exact-match arm above.
        (Type::StrRegex(_), Type::Str) => Ok(()),
        (Type::StrRegex(a), Type::StrRegex(b)) if a == b => Ok(()),
        (_, Type::StrRegex(_)) => {
            Err("regex subtype not supported (deferred); narrowing must be identical".into())
        }

        // literals are subtypes of the primitive/range they satisfy
        (Type::LitStr(_), Type::Str) => Ok(()),
        (Type::LitStr(a), Type::LitStr(b)) if a == b => Ok(()),
        (Type::LitInt(_), Type::Int) => Ok(()),
        (Type::LitInt(a), Type::LitInt(b)) if a == b => Ok(()),
        (Type::LitBool(_), Type::Bool) => Ok(()),
        (Type::LitBool(a), Type::LitBool(b)) if a == b => Ok(()),

        // records: field-wise covariant, required-ness monotone, no new fields
        (Type::Record { fields: nf, .. }, Type::Record { fields: of, .. }) => {
            for ofield in of {
                match nf.iter().find(|f| f.name == ofield.name) {
                    Some(nfield) => {
                        // required-ness may only increase: old required ⇒ new required
                        if !ofield.optional && nfield.optional {
                            return Err(format!(
                                "field `{}` may not become optional (narrowing only)",
                                ofield.name
                            ));
                        }
                        sub(&nfield.ty, &ofield.ty, env, depth + 1)
                            .map_err(|e| format!("field `{}`: {e}", ofield.name))?;
                    }
                    None => {
                        // dropping a field is only ok if it was optional in Old
                        if !ofield.optional {
                            return Err(format!(
                                "required field `{}` may not be dropped",
                                ofield.name
                            ));
                        }
                    }
                }
            }
            // New may not introduce a field Old lacked
            for nfield in nf {
                if !of.iter().any(|f| f.name == nfield.name) {
                    return Err(format!(
                        "field `{}` is not present in the base type (cannot add fields)",
                        nfield.name
                    ));
                }
            }
            Ok(())
        }

        // map → record narrowing: every New field's type <: the map's value type
        (Type::Record { fields, .. }, Type::Map(ov)) => {
            for f in fields {
                sub(&f.ty, ov, env, depth + 1).map_err(|e| format!("field `{}`: {e}", f.name))?;
            }
            Ok(())
        }
        (Type::Map(nv), Type::Map(ov)) => sub(nv, ov, env, depth + 1),
        (Type::List(n), Type::List(o)) => sub(n, o, env, depth + 1),

        _ => Err("not a subtype of the base".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_subtype, narrowed_schema};
    use mangrove_syntax::{Type, TypeDef, UnitDef, parse_type};
    use mangrove_typed::TypeEnv;

    fn env0() -> TypeEnv {
        TypeEnv::build(&[], &[]).unwrap()
    }
    fn ok(new: &str, old: &str) -> bool {
        is_subtype(
            &parse_type(new).unwrap(),
            &parse_type(old).unwrap(),
            &env0(),
        )
        .is_ok()
    }

    #[test]
    fn interval_containment() {
        assert!(ok("int & >= 1 & <= 10", "int")); // narrower <: int
        assert!(ok("int & >= 1 & <= 10", "int & >= 1 & <= 100")); // contained
        assert!(!ok("int", "int & <= 10")); // int is wider than bounded — not a subtype
        assert!(!ok("int & <= 100", "int & <= 10")); // 100 not <= 10
    }

    #[test]
    fn records_narrow_fieldwise() {
        assert!(ok("{ a: int & >= 0, b: str }", "{ a: int, b: str }")); // narrowed field a
        assert!(!ok("{ a: int }", "{ a: int, b: str }")); // dropped required b
        assert!(ok("{ a: int }", "{ a: int, b?: str }")); // dropped optional b ok
        assert!(!ok("{ a: int, c: str }", "{ a: int }")); // added field c
        assert!(!ok("{ a?: int }", "{ a: int }")); // required → optional forbidden
    }

    #[test]
    fn lists_and_unions() {
        assert!(ok("[ int & >= 0 ]", "[ int ]")); // covariant element
        assert!(ok("\"dev\" | \"prod\"", "\"dev\" | \"staging\" | \"prod\"")); // drop variant
        assert!(!ok("\"qa\"", "\"dev\" | \"prod\"")); // not a member
    }

    #[test]
    fn regex_identical_only() {
        assert!(ok("str & =~ \"^a$\"", "str")); // regex <: str
        assert!(ok("str & =~ \"^a$\"", "str & =~ \"^a$\"")); // identical
        assert!(!ok("str & =~ \"^a$\"", "str & =~ \"^b$\"")); // containment deferred
    }

    #[test]
    fn loosening_is_rejected() {
        assert!(!ok("str", "int")); // unrelated
        assert!(!ok("int", "int & >= 1")); // int wider than refined
    }

    #[test]
    fn decimal_interval_containment() {
        assert!(ok("decimal & >= 0.0 & <= 1.0", "decimal")); // refined <: decimal
        assert!(ok("decimal & >= 0.5", "decimal & >= 0.0")); // contained
        assert!(!ok("decimal", "decimal & <= 1.0")); // wider than bounded
        assert!(!ok("decimal & <= 2.0", "decimal & <= 1.0")); // 2.0 not <= 1.0
    }

    #[test]
    fn literals_unions_and_bytes() {
        assert!(ok("\"x\"", "str")); // LitStr <: str
        assert!(ok("5", "int & >= 1 & <= 10")); // LitInt within the range
        assert!(!ok("5", "int & >= 6")); // LitInt out of range
        assert!(ok("bytes", "bytes")); // exact primitive
        assert!(ok("\"a\" | \"b\"", "str")); // union-on-new: every variant <: str
    }

    #[test]
    fn map_and_record_to_map() {
        let env = env0();
        let map_int = Type::Map(Box::new(Type::Int));
        // record <: map: every field's type <: the map's value type
        let rec = parse_type("{ a: int & >= 0, b: int }").unwrap();
        assert!(is_subtype(&rec, &map_int, &env).is_ok());
        // map <: map is covariant in the value type
        let narrow = Type::Map(Box::new(parse_type("int & >= 0").unwrap()));
        assert!(is_subtype(&narrow, &map_int, &env).is_ok());
        // a field whose type isn't <: the map value is rejected
        assert!(is_subtype(&parse_type("{ a: str }").unwrap(), &map_int, &env).is_err());
    }

    #[test]
    fn named_and_unit_resolution() {
        let types = [TypeDef {
            name: "Pos".into(),
            ty: parse_type("int & >= 1").unwrap(),
            annotations: vec![],
        }];
        let units = [UnitDef {
            name: "Bytes".into(),
            members: vec![("B".into(), 1.into())],
        }];
        let env = TypeEnv::build(&types, &units).unwrap();
        assert!(is_subtype(&Type::Named("Pos".into()), &Type::Int, &env).is_ok()); // named refinement <: int
        assert!(is_subtype(&Type::Named("Bytes".into()), &Type::Int, &env).is_ok()); // unit resolves to int
        assert!(is_subtype(&Type::Named("Nope".into()), &Type::Int, &env).is_err()); // unknown name
    }

    #[test]
    fn deep_nesting_errors_not_overflows() {
        let mut t = Type::Int;
        for _ in 0..200 {
            t = Type::List(Box::new(t));
        }
        assert!(is_subtype(&t, &t, &env0()).is_err()); // hits MAX_DEPTH cleanly
    }

    #[test]
    fn narrowed_schema_builds_and_rejects() {
        let env = env0();
        let base = parse_type("{ a: int, b: str }").unwrap();
        // narrowing field `a` is accepted, and the field actually narrows
        assert!(narrowed_schema(&base, &parse_type("{ a: int & >= 0 }").unwrap(), &env).is_ok());
        // adding a field the base lacks is rejected
        assert!(narrowed_schema(&base, &parse_type("{ c: int }").unwrap(), &env).is_err());
        // a non-record base, or a non-record narrowing, both error
        assert!(narrowed_schema(&Type::Int, &parse_type("{ a: int }").unwrap(), &env).is_err());
        assert!(narrowed_schema(&base, &Type::Int, &env).is_err());
    }
}
