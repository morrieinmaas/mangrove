//! Import resolution (§5.1–5.2): the identity/location/auth split.
//!
//! The document carries identity + intent (`use "infra/k8s/core@v5.0"`); a
//! non-committed `.mangrove/resolvers.toml` carries location; a committed
//! `mangrove.lock` carries the pin (reference → BLAKE3 of the source bytes).
//! M3b.1's backend is a local directory; the load-bearing property is
//! verify-before-eval (fail closed). The git backend is M3b.2.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Namespace → remote location (a local directory in M3b.1), plus the directory
/// the resolvers config was found in (relative `remote`s resolve against it).
#[derive(Debug, Default, Clone)]
pub struct Resolvers {
    map: BTreeMap<String, PathBuf>,
    config_dir: Option<PathBuf>,
}

/// Committed pins: `"<namespace>@<tag>"` → `"b3:<hex>"` (source-bytes hash).
#[derive(Debug, Default, Clone)]
pub struct Lockfile {
    map: BTreeMap<String, String>,
    /// The directory `mangrove.lock` was found in / would be written to.
    pub dir: PathBuf,
}

/// `"b3:" + BLAKE3(bytes)` — the content address of imported *source bytes* (§5.2).
pub fn source_hash(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

/// Walk from `start` upward to the filesystem root, returning the first ancestor
/// containing `rel` (and that ancestor).
fn find_upward(start: &Path, rel: &str) -> Option<(PathBuf, PathBuf)> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(rel);
        if candidate.exists() {
            return Some((candidate, d.to_path_buf()));
        }
        dir = d.parent();
    }
    None
}

impl Resolvers {
    /// Find `.mangrove/resolvers.toml` from `root_dir` upward and load it; an
    /// empty resolver set if none exists.
    pub fn find_and_load(root_dir: &Path) -> Result<Resolvers, String> {
        let Some((cfg, config_dir)) = find_upward(root_dir, ".mangrove/resolvers.toml") else {
            return Ok(Resolvers::default());
        };
        let text = std::fs::read_to_string(&cfg).map_err(|e| format!("{}: {e}", cfg.display()))?;
        let table: toml::Table = text
            .parse()
            .map_err(|e| format!("{}: {e}", cfg.display()))?;
        let mut map = BTreeMap::new();
        if let Some(toml::Value::Table(namespaces)) = table.get("namespace") {
            for (ns, v) in namespaces {
                if let toml::Value::Table(entry) = v
                    && let Some(toml::Value::String(remote)) = entry.get("remote")
                {
                    map.insert(ns.clone(), PathBuf::from(remote));
                }
            }
        }
        Ok(Resolvers {
            map,
            config_dir: Some(config_dir),
        })
    }

    /// Resolve a namespaced reference `"<ns>/<rest>@<tag>"` to a local file path.
    /// `<tag>` is only a lockfile key in M3b.1 (the git ref in M3b.2).
    pub fn resolve_path(&self, reference: &str) -> Result<PathBuf, String> {
        let (ns_path, _tag) = reference.rsplit_once('@').ok_or_else(|| {
            format!("namespaced import `{reference}` must be `<namespace>@<tag>`")
        })?;
        let (first, rest) = match ns_path.split_once('/') {
            Some((a, b)) => (a, b),
            None => (ns_path, ""),
        };
        let remote = self.map.get(first).ok_or_else(|| {
            format!("no resolver for namespace `{first}` (.mangrove/resolvers.toml)")
        })?;
        let base = match &self.config_dir {
            Some(d) => d.join(remote),
            None => remote.clone(),
        };
        let rel = if rest.is_empty() {
            format!("{first}.mang")
        } else {
            format!("{rest}.mang")
        };
        Ok(base.join(rel))
    }
}

impl Lockfile {
    /// Find `mangrove.lock` from `root_dir` upward and load it; an empty lockfile
    /// (anchored at `root_dir`) if none exists.
    pub fn find_and_load(root_dir: &Path) -> Result<Lockfile, String> {
        match find_upward(root_dir, "mangrove.lock") {
            Some((file, dir)) => {
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                let table: toml::Table = text
                    .parse()
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                let mut map = BTreeMap::new();
                for (k, v) in &table {
                    if let toml::Value::String(h) = v {
                        map.insert(k.clone(), h.clone());
                    }
                }
                Ok(Lockfile { map, dir })
            }
            None => Ok(Lockfile {
                map: BTreeMap::new(),
                dir: root_dir.to_path_buf(),
            }),
        }
    }

    /// Verify `bytes` against the pinned hash for `reference` (§5.2, fail closed).
    pub fn verify(&self, bytes: &[u8], reference: &str) -> Result<(), String> {
        match self.map.get(reference) {
            Some(pinned) if *pinned == source_hash(bytes) => Ok(()),
            Some(_) => Err(format!("integrity check failed: {reference}")),
            None => Err(format!(
                "{reference} not in mangrove.lock; run `mangrove update`"
            )),
        }
    }

    /// Insert/update a pin (used by `mangrove update`).
    pub fn insert(&mut self, reference: String, hash: String) {
        self.map.insert(reference, hash);
    }

    /// Serialize to the committed `mangrove.lock` text (sorted, deterministic).
    /// Uses the `toml` crate so the writer's escaping matches the reader's — a
    /// reference with a control char round-trips instead of emitting invalid TOML.
    pub fn to_toml(&self) -> String {
        let mut table = toml::Table::new();
        for (k, v) in &self.map {
            table.insert(k.clone(), toml::Value::String(v.clone()));
        }
        toml::to_string(&table).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("m3b_resolve_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
    }

    #[test]
    fn source_hash_is_b3_prefixed() {
        let h = source_hash(b"hello");
        assert!(h.starts_with("b3:") && h.len() == 3 + 64);
    }

    #[test]
    fn resolvers_parse_and_resolve() {
        let dir = scratch();
        write(
            &dir.join(".mangrove/resolvers.toml"),
            "[namespace.infra]\nremote = \"vendor/infra\"\n",
        );
        let r = Resolvers::find_and_load(&dir).unwrap();
        let p = r.resolve_path("infra/k8s/core@v5.0").unwrap();
        assert_eq!(p, dir.join("vendor/infra").join("k8s/core.mang"));
    }

    #[test]
    fn unknown_namespace_errors() {
        let r = Resolvers::default();
        assert!(r.resolve_path("nope/x@v1").is_err());
    }

    #[test]
    fn missing_at_tag_errors() {
        let dir = scratch();
        write(
            &dir.join(".mangrove/resolvers.toml"),
            "[namespace.infra]\nremote = \"x\"\n",
        );
        let r = Resolvers::find_and_load(&dir).unwrap();
        assert!(r.resolve_path("infra/k8s/core").is_err()); // no @tag
    }

    #[test]
    fn lockfile_verify_match_mismatch_missing() {
        let dir = scratch();
        let bytes = b"name: \"x\"\n";
        write(
            &dir.join("mangrove.lock"),
            &format!("\"infra/x@v1\" = {:?}\n", source_hash(bytes)),
        );
        let lock = Lockfile::find_and_load(&dir).unwrap();
        assert!(lock.verify(bytes, "infra/x@v1").is_ok());
        assert!(lock.verify(b"tampered", "infra/x@v1").is_err()); // mismatch
        assert!(lock.verify(bytes, "infra/y@v1").is_err()); // missing
    }

    #[test]
    fn lock_roundtrips_to_toml() {
        let mut lock = Lockfile::default();
        lock.insert("infra/x@v1".into(), "b3:abc".into());
        // a control char in the reference must still round-trip (S4: `{:?}`
        // escaping emitted invalid TOML here; the toml crate does not).
        lock.insert("a\u{1}b@v1".into(), "b3:def".into());
        let text = lock.to_toml();
        let reparsed: toml::Table = text.parse().unwrap();
        assert_eq!(
            reparsed.get("infra/x@v1").and_then(|v| v.as_str()),
            Some("b3:abc")
        );
        assert_eq!(
            reparsed.get("a\u{1}b@v1").and_then(|v| v.as_str()),
            Some("b3:def")
        );
    }
}
