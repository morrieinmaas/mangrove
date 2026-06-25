//! Local `use` resolution + the compose driver (§5.1, §5.3). Loads a document
//! and its local `use`d siblings (recursive, cycle-checked), then folds the
//! body statements (spreads + binds) left-to-right via the merge engine into a
//! single value — `later wins`, records deep-merge, `unset` removes.

use crate::merge::merge;
use mangrove_core::Value;
use mangrove_resolve::{Lockfile, Resolvers};
use mangrove_syntax::{
    Annotation, Document, FnDef, ListOpItem, Param, Stmt, Type, TypeDef, UnitDef, parse_document,
};
use mangrove_typed::TypeEnv;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bound on `use`-chain depth — guards against stack overflow on a deep chain
/// of local imports (cf. the parser/merge `MAX_DEPTH`). Far beyond any real
/// project's layering.
const MAX_USE_DEPTH: usize = 128;

/// A composed document: the merged body plus the importing document's own
/// type/unit/schema declarations (used downstream for resolve + validate). It
/// mirrors the fields the CLI used from a parsed `Document`.
#[derive(Debug)]
pub struct Composed {
    pub typedefs: Vec<TypeDef>,
    pub unitdefs: Vec<UnitDef>,
    pub schema: Option<String>,
    /// `schema Base & { … }` narrowing record, if present (§5.5).
    pub schema_narrow: Option<Type>,
    /// L3 `params` of the root document (§6.1); govern the eval stage.
    pub params: Vec<Param>,
    /// L3 schema-defined functions of the root document (§6.2).
    pub fns: Vec<FnDef>,
    /// Direct `use` aliases → their composed module, for module calls (§6.1, M4d.2).
    pub modules: BTreeMap<String, Composed>,
    pub body: Value,
}

/// Compose the document at `path`, resolving local `use`s, spreads, and — via
/// the resolver + lockfile (`.mangrove/resolvers.toml`, `mangrove.lock`) —
/// namespaced `use`s (hash-verified before evaluation, §5.2).
pub fn compose(path: &Path) -> Result<Composed, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let root_dir = canon.parent().unwrap_or_else(|| Path::new("."));
    let resolvers = Resolvers::find_and_load(root_dir)?;
    let lock = Lockfile::find_and_load(root_dir)?;
    compose_rec(path, &mut Vec::new(), &resolvers, &lock, false, None)
}

/// `in_remote`: true once we have crossed a namespaced import — within a remote
/// subtree every file is pinned and unpinnable local (`./`) imports are refused.
/// `verify_as`: the lockfile reference to verify *this* file's bytes against
/// (the bytes are read once and that exact buffer is both verified and parsed —
/// no re-open, so no TOCTOU).
fn compose_rec(
    path: &Path,
    visiting: &mut Vec<PathBuf>,
    resolvers: &Resolvers,
    lock: &Lockfile,
    in_remote: bool,
    verify_as: Option<&str>,
) -> Result<Composed, String> {
    // `visiting.len()` is the current `use`-chain depth; cap it so a deep chain
    // of files errors cleanly instead of overflowing the stack (SIGABRT).
    if visiting.len() >= MAX_USE_DEPTH {
        return Err(format!("`use` chain too deep (> {MAX_USE_DEPTH})"));
    }
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if visiting.contains(&canon) {
        return Err(format!("cyclic `use` involving {}", path.display()));
    }
    // Read once; verify THIS buffer before parsing it (no second open → no TOCTOU).
    let bytes = std::fs::read(&canon).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(reference) = verify_as {
        lock.verify(&bytes, reference)?;
    }
    let src = String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8", path.display()))?;
    let doc = parse_document(&src).map_err(|e| format!("{}:{e}", path.display()))?;

    visiting.push(canon.clone());
    let dir = canon.parent().unwrap_or_else(|| Path::new("."));
    // alias → the full sub-`Composed`: its `.body` feeds a spread (`...alias`),
    // and the whole module feeds a module call (`alias(args)`, M4d.2).
    let mut modules: BTreeMap<String, Composed> = BTreeMap::new();
    for u in &doc.uses {
        let base = if u.path.starts_with("./") || u.path.starts_with("../") {
            // Local import. Inside a remote subtree it is unpinnable → refuse it,
            // so a verified package cannot smuggle in unverified content (B1).
            if in_remote {
                return Err(format!(
                    "a remote package may not use the local import `{}` (unpinnable; M3b.1)",
                    u.path
                ));
            }
            compose_rec(&dir.join(&u.path), visiting, resolvers, lock, false, None)?
        } else {
            // Namespaced: resolve, then the recursive call verifies the bytes it
            // reads against the lockfile before evaluating them (fail closed).
            let resolved = resolvers.resolve_path(&u.path)?;
            compose_rec(&resolved, visiting, resolvers, lock, true, Some(&u.path))?
        };
        modules.insert(u.alias.clone(), base);
    }
    visiting.pop();

    // Fold the body statements left-to-right.
    let mut acc = Value::Map(BTreeMap::new());
    for stmt in &doc.stmts {
        match stmt {
            Stmt::Spread(alias) => {
                let base = modules
                    .get(alias)
                    .ok_or_else(|| format!("unknown spread alias `{alias}`"))?;
                acc = merge(acc, base.body.clone());
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
        schema_narrow: doc.schema_narrow,
        params: doc.params,
        fns: doc.fns,
        modules,
        body: acc,
    })
}

/// Walk the `use` graph reachable from `path` and return every namespaced
/// reference mapped to its source-bytes hash (for `mangrove update`). Unlike
/// composing, this does NOT verify against the lockfile — it *produces* it.
pub fn lock_references(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let root_dir = canon.parent().unwrap_or_else(|| Path::new("."));
    let resolvers = Resolvers::find_and_load(root_dir)?;
    let mut out = BTreeMap::new();
    collect_rec(&canon, &resolvers, &mut Vec::new(), &mut out, false)?;
    Ok(out)
}

fn collect_rec(
    path: &Path,
    resolvers: &Resolvers,
    visiting: &mut Vec<PathBuf>,
    out: &mut BTreeMap<String, String>,
    in_remote: bool,
) -> Result<(), String> {
    if visiting.len() >= MAX_USE_DEPTH {
        return Err(format!("`use` chain too deep (> {MAX_USE_DEPTH})"));
    }
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if visiting.contains(&canon) {
        return Err(format!("cyclic `use` involving {}", path.display()));
    }
    let src = std::fs::read_to_string(&canon).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = parse_document(&src).map_err(|e| format!("{}:{e}", path.display()))?;
    visiting.push(canon.clone());
    let dir = canon.parent().unwrap_or_else(|| Path::new("."));
    for u in &doc.uses {
        // Mirror compose_rec: a `./` import inside a remote subtree is unpinnable,
        // so refuse it here too — the lock we write must match what compose accepts.
        if u.path.starts_with("./") || u.path.starts_with("../") {
            if in_remote {
                return Err(format!(
                    "a remote package may not use the local import `{}` (unpinnable; M3b.1)",
                    u.path
                ));
            }
            collect_rec(&dir.join(&u.path), resolvers, visiting, out, false)?;
        } else {
            let p = resolvers.resolve_path(&u.path)?;
            let bytes = std::fs::read(&p)
                .map_err(|e| format!("resolving `{}`: {}: {e}", u.path, p.display()))?;
            out.insert(u.path.clone(), mangrove_resolve::source_hash(&bytes));
            collect_rec(&p, resolvers, visiting, out, true)?;
        }
    }
    visiting.pop();
    Ok(())
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
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut f = std::fs::File::create(&path).unwrap();
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
    fn namespaced_use_without_resolver_errors() {
        // No .mangrove/resolvers.toml → the namespace can't be resolved.
        let dir = scratch(&[("d.mang", "use \"infra/k8s/core@v1\" as k\n...k\n")]);
        let e = compose(&dir.join("d.mang")).unwrap_err();
        assert!(e.contains("resolver"), "{e}");
    }

    #[test]
    fn namespaced_use_verified_composes() {
        let dir = scratch(&[
            ("vendor/x.mang", "name: \"shared\"\nport: 80\n"),
            ("root.mang", "use \"infra/x@v1\" as k\n...k\nport: 9090\n"),
            (
                ".mangrove/resolvers.toml",
                "[namespace.infra]\nremote = \"vendor\"\n",
            ),
        ]);
        // pin the correct hash of vendor/x.mang
        let bytes = std::fs::read(dir.join("vendor/x.mang")).unwrap();
        std::fs::write(
            dir.join("mangrove.lock"),
            format!(
                "\"infra/x@v1\" = {:?}\n",
                mangrove_resolve::source_hash(&bytes)
            ),
        )
        .unwrap();
        let c = compose(&dir.join("root.mang")).unwrap();
        assert_eq!(get(&c.body, "name"), Some(&Value::Str("shared".into())));
        assert_eq!(get(&c.body, "port"), Some(&Value::Int(9090.into()))); // override
    }

    #[test]
    fn tampered_source_fails_integrity() {
        let dir = scratch(&[
            ("vendor/x.mang", "name: \"shared\"\n"),
            ("root.mang", "use \"infra/x@v1\" as k\n...k\n"),
            (
                ".mangrove/resolvers.toml",
                "[namespace.infra]\nremote = \"vendor\"\n",
            ),
            // lock pins a hash that won't match the actual source
            ("mangrove.lock", "\"infra/x@v1\" = \"b3:deadbeef\"\n"),
        ]);
        let e = compose(&dir.join("root.mang")).unwrap_err();
        assert!(e.contains("integrity check failed"), "{e}");
    }

    fn git(args: &[&str], cwd: &std::path::Path) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    #[test]
    fn git_backend_composes_with_verified_lock() {
        // End-to-end: a namespaced import served from a git repo is hash-verified
        // against the lock before composing — proving D27 holds for git bytes.
        let content = "name: \"shared\"\nport: 80\n";
        let repo = scratch(&[]).join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&["init", "--quiet"], &repo);
        std::fs::write(repo.join("x.mang"), content).unwrap();
        git(&["add", "."], &repo);
        git(&["commit", "--quiet", "--no-verify", "-m", "init"], &repo);
        git(&["tag", "v1"], &repo);

        let resolvers = format!("[namespace.pkg]\ngit = {:?}\n", repo.to_str().unwrap());
        let lock = format!(
            "\"pkg/x@v1\" = {:?}\n",
            mangrove_resolve::source_hash(content.as_bytes())
        );
        let dir = scratch(&[
            ("root.mang", "use \"pkg/x@v1\" as k\n...k\nport: 9090\n"),
            (".mangrove/resolvers.toml", resolvers.as_str()),
            ("mangrove.lock", lock.as_str()),
        ]);
        let c = compose(&dir.join("root.mang")).unwrap();
        assert_eq!(get(&c.body, "name"), Some(&Value::Str("shared".into())));
        assert_eq!(get(&c.body, "port"), Some(&Value::Int(9090.into())));
    }

    #[test]
    fn remote_package_local_import_is_refused() {
        // B1: a pinned entry file must not smuggle in an unpinned `./` sibling.
        let dir = scratch(&[
            ("vendor/x.mang", "use \"./secret.mang\" as s\n...s\n"),
            ("vendor/secret.mang", "injected: \"PAYLOAD\"\n"), // NOT in the lock
            ("root.mang", "use \"infra/x@v1\" as k\n...k\n"),
            (
                ".mangrove/resolvers.toml",
                "[namespace.infra]\nremote = \"vendor\"\n",
            ),
        ]);
        let bytes = std::fs::read(dir.join("vendor/x.mang")).unwrap();
        std::fs::write(
            dir.join("mangrove.lock"),
            format!(
                "\"infra/x@v1\" = {:?}\n",
                mangrove_resolve::source_hash(&bytes)
            ),
        )
        .unwrap();
        let e = compose(&dir.join("root.mang")).unwrap_err();
        assert!(e.contains("remote package may not use"), "{e}");
    }

    #[test]
    fn missing_lock_entry_errors() {
        let dir = scratch(&[
            ("vendor/x.mang", "name: \"shared\"\n"),
            ("root.mang", "use \"infra/x@v1\" as k\n...k\n"),
            (
                ".mangrove/resolvers.toml",
                "[namespace.infra]\nremote = \"vendor\"\n",
            ),
            // no mangrove.lock
        ]);
        let e = compose(&dir.join("root.mang")).unwrap_err();
        assert!(e.contains("mangrove update"), "{e}");
    }

    #[test]
    fn deep_use_chain_errors_instead_of_overflowing() {
        // Was: SIGABRT via unbounded compose_rec recursion. Now a clean error.
        let n = 300usize; // > MAX_USE_DEPTH(128); shallow enough to build fast
        let files: Vec<(String, String)> = (0..n)
            .map(|i| {
                let name = format!("f{i}.mang");
                let body = if i + 1 < n {
                    format!("use \"./f{}.mang\" as nxt\n...nxt\n", i + 1)
                } else {
                    "leaf: 1\n".to_string()
                };
                (name, body)
            })
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        let dir = scratch(&refs);
        let e = compose(&dir.join("f0.mang")).unwrap_err();
        assert!(e.contains("too deep"), "{e}");
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
