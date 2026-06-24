//! Local `use` resolution + the compose driver (§5.1, §5.3). Loads a document
//! and its local `use`d siblings (recursive, cycle-checked), then folds the
//! body statements (spreads + binds) left-to-right via the merge engine into a
//! single value — `later wins`, records deep-merge, `unset` removes.

use crate::merge::merge;
use mangrove_core::Value;
use mangrove_syntax::{Stmt, TypeDef, UnitDef, parse_document};
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
        }
    }

    Ok(Composed {
        typedefs: doc.typedefs,
        unitdefs: doc.unitdefs,
        schema: doc.schema,
        body: acc,
    })
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
}
