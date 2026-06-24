//! Local `use` resolution + the compose driver (§5.1, §5.3). Loads a document
//! and its local `use`d siblings (recursive, cycle-checked), then folds the
//! body statements (spreads + binds) left-to-right via the merge engine into a
//! single value — `later wins`, records deep-merge, `unset` removes.

use crate::merge::merge;
use mangrove_core::Value;
use mangrove_syntax::{
    Annotation, Document, ListOpItem, Stmt, Type, TypeDef, UnitDef, parse_document,
};
use mangrove_typed::TypeEnv;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A composed document: the merged body plus the importing document's own
/// type/unit/schema declarations (used downstream for resolve + validate). It
/// mirrors the fields the CLI used from a parsed `Document`.
#[derive(Debug)]
pub struct Composed {
    pub typedefs: Vec<TypeDef>,
    pub unitdefs: Vec<UnitDef>,
    pub schema: Option<String>,
    pub body: Value,
}

/// Compose the document at `path` (resolving local `use`s and spreads).
pub fn compose(path: &Path) -> Result<Composed, String> {
    compose_rec(path, &mut Vec::new())
}

fn compose_rec(path: &Path, visiting: &mut Vec<PathBuf>) -> Result<Composed, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if visiting.contains(&canon) {
        return Err(format!("cyclic `use` involving {}", path.display()));
    }
    let src = std::fs::read_to_string(&canon).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = parse_document(&src).map_err(|e| format!("{}:{e}", path.display()))?;

    // Resolve each local `use` to its composed body, keyed by alias.
    visiting.push(canon.clone());
    let dir = canon.parent().unwrap_or_else(|| Path::new("."));
    let mut bases: BTreeMap<String, Value> = BTreeMap::new();
    for u in &doc.uses {
        if !(u.path.starts_with("./") || u.path.starts_with("../")) {
            visiting.pop();
            return Err(format!(
                "remote/namespaced import `{}` requires a resolver (M3b); use a relative path",
                u.path
            ));
        }
        let base = compose_rec(&dir.join(&u.path), visiting)?;
        bases.insert(u.alias.clone(), base.body);
    }
    visiting.pop();

    // Fold the body statements left-to-right.
    let mut acc = Value::Map(BTreeMap::new());
    for stmt in &doc.stmts {
        match stmt {
            Stmt::Spread(alias) => {
                let base = bases
                    .get(alias)
                    .ok_or_else(|| format!("unknown spread alias `{alias}`"))?;
                acc = merge(acc, base.clone());
            }
            Stmt::Bind(k, v) => {
                let mut one = BTreeMap::new();
                one.insert(k.clone(), v.clone());
                acc = merge(acc, Value::Map(one));
            }
            Stmt::Append(k, v) => {
                acc = apply_append(acc, k, v.clone())?;
            }
            Stmt::ListOp(k, items) => {
                let keyfield = key_field(&doc, k)?;
                acc = apply_list_ops(acc, k, items, &keyfield)?;
            }
        }
    }

    Ok(Composed {
        typedefs: doc.typedefs,
        unitdefs: doc.unitdefs,
        schema: doc.schema,
        body: acc,
    })
}

/// `key += [ … ]` — append a list to the inherited list (D22).
fn apply_append(acc: Value, k: &str, v: Value) -> Result<Value, String> {
    let Value::List(add) = v else {
        return Err(format!("`{k} += …` requires a list on the right"));
    };
    let Value::Map(mut m) = acc else {
        return Ok(Value::Map(BTreeMap::new()));
    };
    let mut list = match m.remove(k) {
        Some(Value::List(l)) => l,
        None => Vec::new(),
        Some(_) => return Err(format!("`{k}` is not a list; cannot use `+=`")),
    };
    list.extend(add);
    m.insert(k.to_string(), Value::List(list));
    Ok(Value::Map(m))
}

/// Apply a `@key` list-op block (patch/append/remove) to the inherited list,
/// matching elements by their `keyfield` value (D22).
fn apply_list_ops(
    acc: Value,
    k: &str,
    items: &[ListOpItem],
    keyfield: &str,
) -> Result<Value, String> {
    let Value::Map(mut m) = acc else {
        return Ok(Value::Map(BTreeMap::new()));
    };
    let mut list = match m.remove(k) {
        Some(Value::List(l)) => l,
        None => Vec::new(),
        Some(_) => return Err(format!("`{k}` is not a list; cannot apply list ops")),
    };
    for item in items {
        match item {
            ListOpItem::Patch(key, val) => match find_by_key(&list, keyfield, key) {
                Some(idx) => {
                    let elem = list.remove(idx);
                    list.insert(idx, merge(elem, val.clone()));
                }
                None => return Err(format!("patch: no element with {keyfield} == {key:?}")),
            },
            ListOpItem::Append(val) => {
                let nk = elem_key(val, keyfield)?;
                if find_by_key(&list, keyfield, &nk).is_some() {
                    return Err(format!(
                        "append: element with {keyfield} == {nk:?} already exists"
                    ));
                }
                list.push(val.clone());
            }
            ListOpItem::Remove(key) => match find_by_key(&list, keyfield, key) {
                Some(idx) => {
                    list.remove(idx);
                }
                None => return Err(format!("remove: no element with {keyfield} == {key:?}")),
            },
        }
    }
    m.insert(k.to_string(), Value::List(list));
    Ok(Value::Map(m))
}

/// Index of the list element that is a map with `map[keyfield] == Str(key)`.
fn find_by_key(list: &[Value], keyfield: &str, key: &str) -> Option<usize> {
    list.iter().position(|e| match e {
        Value::Map(m) => matches!(m.get(keyfield), Some(Value::Str(s)) if s == key),
        _ => false,
    })
}

fn elem_key(val: &Value, keyfield: &str) -> Result<String, String> {
    match val {
        Value::Map(m) => match m.get(keyfield) {
            Some(Value::Str(s)) => Ok(s.clone()),
            _ => Err(format!("appended element lacks a string `{keyfield}` key")),
        },
        _ => Err("appended element must be a record".into()),
    }
}

/// Resolve the `@key(field)` of a top-level list field from the document's local
/// schema (M3a; cross-file schemas are M3b).
fn key_field(doc: &Document, field: &str) -> Result<String, String> {
    let schema = doc
        .schema
        .as_ref()
        .ok_or_else(|| format!("list ops on `{field}` need a schema declaring `@key`"))?;
    let env = TypeEnv::build(&doc.typedefs, &doc.unitdefs)?;
    let ty = env
        .resolve(schema)
        .ok_or_else(|| format!("unknown schema type `{schema}`"))?;
    let Type::Record { fields, .. } = ty else {
        return Err(format!("schema `{schema}` is not a record"));
    };
    let fd = fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| format!("schema `{schema}` has no field `{field}`"))?;
    Annotation::find(&fd.annotations, "key")
        .map(str::to_string)
        .ok_or_else(|| format!("field `{field}` has no `@key` annotation"))
}

#[cfg(test)]
mod tests {
    use super::compose;
    use mangrove_core::Value;
    use std::io::Write;

    /// Write `files` (name → contents) into a fresh temp dir; return its path.
    fn scratch(files: &[(&str, &str)]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("m3a_compose_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, contents) in files {
            let mut f = std::fs::File::create(dir.join(name)).unwrap();
            f.write_all(contents.as_bytes()).unwrap();
        }
        dir
    }

    fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
        match v {
            Value::Map(m) => m.get(k),
            _ => None,
        }
    }

    #[test]
    fn spread_then_override() {
        let dir = scratch(&[
            ("base.mang", "name: \"x\"\nport: 8080\n"),
            (
                "over.mang",
                "use \"./base.mang\" as base\n...base\nport: 9090\n",
            ),
        ]);
        let c = compose(&dir.join("over.mang")).unwrap();
        assert_eq!(get(&c.body, "name"), Some(&Value::Str("x".into())));
        assert_eq!(get(&c.body, "port"), Some(&Value::Int(9090.into()))); // override
    }

    #[test]
    fn spread_then_unset_removes_inherited() {
        let dir = scratch(&[
            ("base.mang", "a: 1\nb: 2\n"),
            (
                "over.mang",
                "use \"./base.mang\" as base\n...base\nb: unset\n",
            ),
        ]);
        let c = compose(&dir.join("over.mang")).unwrap();
        assert_eq!(get(&c.body, "a"), Some(&Value::Int(1.into())));
        assert_eq!(get(&c.body, "b"), None); // removed
    }

    #[test]
    fn cyclic_use_errors() {
        let dir = scratch(&[
            ("a.mang", "use \"./b.mang\" as b\n...b\n"),
            ("b.mang", "use \"./a.mang\" as a\n...a\n"),
        ]);
        assert!(compose(&dir.join("a.mang")).is_err());
    }

    #[test]
    fn remote_import_errors_in_m3a() {
        let dir = scratch(&[("d.mang", "use \"infra/k8s/core\" as k\n...k\n")]);
        let e = compose(&dir.join("d.mang")).unwrap_err();
        assert!(e.contains("resolver"));
    }

    #[test]
    fn append_op_extends_inherited_list() {
        let dir = scratch(&[
            ("base.mang", "ports: [ 80 ]\n"),
            (
                "over.mang",
                "use \"./base.mang\" as base\n...base\nports += [ 443 ]\n",
            ),
        ]);
        let c = compose(&dir.join("over.mang")).unwrap();
        assert_eq!(
            get(&c.body, "ports"),
            Some(&Value::List(vec![
                Value::Int(80.into()),
                Value::Int(443.into())
            ]))
        );
    }

    #[test]
    fn key_list_ops_patch_append_remove() {
        let schema = "type C = { name: str, image: str }\n\
                      type D = { containers: [ C ] @key(name) }\nschema D\n";
        let over = format!(
            "use \"./base.mang\" as base\n{schema}...base\n\
             containers {{ patch \"api\": {{ image: \"api:2.0\" }}, append: {{ name: \"envoy\", image: \"envoy:1\" }}, remove: \"cron\" }}\n"
        );
        let dir = scratch(&[
            (
                "base.mang",
                "containers: [ { name: \"api\", image: \"api:1.0\" }, { name: \"cron\", image: \"cron:1\" } ]\n",
            ),
            ("over.mang", over.as_str()),
        ]);
        let c = compose(&dir.join("over.mang")).unwrap();
        let Some(Value::List(list)) = get(&c.body, "containers") else {
            panic!()
        };
        let names: Vec<&str> = list
            .iter()
            .filter_map(|e| match e {
                Value::Map(m) => match m.get("name") {
                    Some(Value::Str(s)) => Some(s.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["api", "envoy"]); // cron removed, envoy appended
        // api was patched
        let api = &list[0];
        assert_eq!(
            match api {
                Value::Map(m) => m.get("image"),
                _ => None,
            },
            Some(&Value::Str("api:2.0".into()))
        );
    }
}
