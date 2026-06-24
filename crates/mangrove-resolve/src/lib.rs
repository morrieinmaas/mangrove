//! Import resolution (§5.1–5.2): the identity/location/auth split.
//!
//! The document carries identity + intent (`use "infra/k8s/core@v5.0"`); a
//! non-committed `.mangrove/resolvers.toml` carries location; a committed
//! `mangrove.lock` carries the pin (reference → BLAKE3 of the source bytes).
//! M3b.1's backend is a local directory; the load-bearing property is
//! verify-before-eval (fail closed). The git backend is M3b.2.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a namespace's sources live (§5.1 "location").
#[derive(Debug, Clone)]
enum Backend {
    /// A local directory (M3b.1), relative to the resolvers config dir.
    Local(PathBuf),
    /// A git repository (M3b.2); `<tag>` is the git ref, checked out into a cache.
    Git { url: String },
}

/// Namespace → backend, plus the directory the resolvers config was found in
/// (relative locations and the git cache resolve against it).
#[derive(Debug, Default, Clone)]
pub struct Resolvers {
    map: BTreeMap<String, Backend>,
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
                let toml::Value::Table(entry) = v else {
                    continue;
                };
                let remote = entry.get("remote").and_then(toml::Value::as_str);
                let git = entry.get("git").and_then(toml::Value::as_str);
                let backend = match (remote, git) {
                    (Some(r), None) => Backend::Local(PathBuf::from(r)),
                    (None, Some(g)) => Backend::Git { url: g.to_string() },
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "{}: namespace `{ns}` sets both `remote` and `git` (pick one)",
                            cfg.display()
                        ));
                    }
                    (None, None) => {
                        return Err(format!(
                            "{}: namespace `{ns}` needs `remote` or `git`",
                            cfg.display()
                        ));
                    }
                };
                map.insert(ns.clone(), backend);
            }
        }
        Ok(Resolvers {
            map,
            config_dir: Some(config_dir),
        })
    }

    /// Resolve a namespaced reference `"<ns>/<rest>@<tag>"` to a local file path.
    /// For a git backend this fetches the repo at `<tag>` into the cache first.
    /// The returned bytes are still hash-verified by the caller (D27).
    pub fn resolve_path(&self, reference: &str) -> Result<PathBuf, String> {
        let (ns_path, tag) = reference.rsplit_once('@').ok_or_else(|| {
            format!("namespaced import `{reference}` must be `<namespace>@<tag>`")
        })?;
        let (first, rest) = match ns_path.split_once('/') {
            Some((a, b)) => (a, b),
            None => (ns_path, ""),
        };
        // D32: reject `..`, leading `-`, and anything outside a safe charset — for
        // both backends, closing path-escape (rest) and git arg-injection (tag).
        validate_component(first, "namespace")?;
        validate_component(tag, "tag")?;
        if !rest.is_empty() {
            validate_component(rest, "path")?;
        }
        let backend = self.map.get(first).ok_or_else(|| {
            format!("no resolver for namespace `{first}` (.mangrove/resolvers.toml)")
        })?;
        let rel = if rest.is_empty() {
            format!("{first}.mang")
        } else {
            format!("{rest}.mang")
        };
        match backend {
            Backend::Local(remote) => {
                let base = match &self.config_dir {
                    Some(d) => d.join(remote),
                    None => remote.clone(),
                };
                Ok(base.join(rel))
            }
            Backend::Git { url } => {
                let cfg = self
                    .config_dir
                    .as_ref()
                    .ok_or("git backend needs a config directory")?;
                let checkout = cfg.join(".mangrove/cache").join(first).join(tag);
                git_fetch(url, tag, &checkout)?;
                Ok(checkout.join(rel))
            }
        }
    }
}

/// Reject empty/`..`/leading-`-` components and anything outside `[A-Za-z0-9._/-]`.
fn validate_component(s: &str, what: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && !s.starts_with('-')
        && s.split('/').all(|seg| !seg.is_empty() && seg != "..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid {what} `{s}` in namespaced import"))
    }
}

/// Clone `url` and check out `git_ref` into `checkout` (once; reused if present).
/// Never goes through a shell; `url` is passed after `--` so it cannot be read as
/// an option, and `git_ref` is pre-validated by [`validate_component`] (D32).
fn git_fetch(url: &str, git_ref: &str, checkout: &Path) -> Result<(), String> {
    if checkout.exists() {
        return Ok(()); // cached (D31): one clone per (namespace, ref)
    }
    if let Some(parent) = checkout.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("git cache dir: {e}"))?;
    }
    let run = |args: &[&std::ffi::OsStr]| -> Result<(), String> {
        let out = std::process::Command::new("git")
            .args(args)
            .output()
            .map_err(|e| format!("running git (is it installed?): {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let _ = std::fs::remove_dir_all(checkout); // don't leave a half clone cached
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    use std::ffi::OsStr;
    run(&[
        OsStr::new("clone"),
        OsStr::new("--quiet"),
        OsStr::new("--"),
        OsStr::new(url),
        checkout.as_os_str(),
    ])
    .map_err(|e| format!("git clone {url} failed: {e}"))?;
    run(&[
        OsStr::new("-C"),
        checkout.as_os_str(),
        OsStr::new("-c"),
        OsStr::new("advice.detachedHead=false"),
        OsStr::new("checkout"),
        OsStr::new("--quiet"),
        OsStr::new(git_ref),
        OsStr::new("--"),
    ])
    .map_err(|e| format!("git checkout {git_ref} failed: {e}"))?;
    Ok(())
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

    // ---- M3b.2 git backend (hermetic: a local git repo stands in for a remote) ----

    fn git(args: &[&str], cwd: &Path) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Init a repo with one file committed and tagged `v1`; return its path.
    fn make_git_repo(file: &str, contents: &str) -> PathBuf {
        let repo = scratch().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&["init", "--quiet"], &repo);
        write(&repo.join(file), contents);
        git(&["add", "."], &repo);
        git(&["commit", "--quiet", "--no-verify", "-m", "init"], &repo);
        git(&["tag", "v1"], &repo);
        repo
    }

    fn git_project(repo: &Path) -> (PathBuf, Resolvers) {
        let proj = scratch();
        write(
            &proj.join(".mangrove/resolvers.toml"),
            &format!("[namespace.pkg]\ngit = {:?}\n", repo.to_str().unwrap()),
        );
        let r = Resolvers::find_and_load(&proj).unwrap();
        (proj, r)
    }

    #[test]
    fn git_backend_resolves_and_reads() {
        let repo = make_git_repo("x.mang", "name: \"git\"\n");
        let (_proj, r) = git_project(&repo);
        let p = r.resolve_path("pkg/x@v1").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "name: \"git\"\n");
    }

    #[test]
    fn git_backend_caches_clone() {
        let repo = make_git_repo("x.mang", "v: 1\n");
        let (_proj, r) = git_project(&repo);
        assert!(r.resolve_path("pkg/x@v1").unwrap().exists());
        std::fs::remove_dir_all(&repo).unwrap(); // remote gone…
        assert!(r.resolve_path("pkg/x@v1").unwrap().exists()); // …still served from cache
    }

    #[test]
    fn git_backend_bad_ref_errors() {
        let repo = make_git_repo("x.mang", "v: 1\n");
        let (_proj, r) = git_project(&repo);
        assert!(r.resolve_path("pkg/x@nope").is_err());
    }

    #[test]
    fn git_source_still_goes_through_verify() {
        let repo = make_git_repo("x.mang", "name: \"v\"\n");
        let (_proj, r) = git_project(&repo);
        let bytes = std::fs::read(r.resolve_path("pkg/x@v1").unwrap()).unwrap();
        let mut wrong = Lockfile::default();
        wrong.insert("pkg/x@v1".into(), "b3:deadbeef".into());
        assert!(wrong.verify(&bytes, "pkg/x@v1").is_err());
        let mut right = Lockfile::default();
        right.insert("pkg/x@v1".into(), source_hash(&bytes));
        assert!(right.verify(&bytes, "pkg/x@v1").is_ok());
    }

    #[test]
    fn rejects_unsafe_reference_components() {
        let dir = scratch();
        write(
            &dir.join(".mangrove/resolvers.toml"),
            "[namespace.pkg]\ngit = \"u\"\n",
        );
        let r = Resolvers::find_and_load(&dir).unwrap();
        assert!(r.resolve_path("pkg/../escape@v1").is_err()); // `..` path escape
        assert!(r.resolve_path("pkg/x@-flag").is_err()); // git arg-injection via tag
        assert!(r.resolve_path("pkg/x@v 1").is_err()); // space outside charset
    }

    #[test]
    fn backend_both_or_neither_errors() {
        let a = scratch();
        write(
            &a.join(".mangrove/resolvers.toml"),
            "[namespace.a]\nremote = \"d\"\ngit = \"u\"\n",
        );
        assert!(Resolvers::find_and_load(&a).is_err());
        let b = scratch();
        write(&b.join(".mangrove/resolvers.toml"), "[namespace.a]\n");
        assert!(Resolvers::find_and_load(&b).is_err());
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
