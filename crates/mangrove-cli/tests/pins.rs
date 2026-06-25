//! M6b: per-type version pins (§5.6) — `k.Probe @"v1"` validates that slot
//! against version v1 of the package, overriding the version `k` was `use`d at.
//! Pins are meaningful only with the git backend (a tag is a real ref); the
//! local backend ignores the tag. Hermetic: a local git repo with two tags.

use std::process::Command;

fn git(args: &[&str], cwd: &std::path::Path) {
    let out = Command::new("git")
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

fn mangrove(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(args)
        .output()
        .expect("run")
}

#[test]
fn per_type_version_pin_selects_the_pinned_version() {
    let base = std::env::temp_dir().join(format!("m6b_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let proj = base.join("proj");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(proj.join(".mangrove")).unwrap();

    // v1: Probe unbounded; v2: Probe with port <= 100.
    git(&["init", "-q"], &repo);
    std::fs::write(repo.join("probe.mang"), "type Probe = { port: int }\n").unwrap();
    git(&["add", "."], &repo);
    git(&["commit", "--no-verify", "-q", "-m", "v1"], &repo);
    git(&["tag", "v1"], &repo);
    std::fs::write(
        repo.join("probe.mang"),
        "type Probe = { port: int & <= 100 }\n",
    )
    .unwrap();
    git(&["add", "."], &repo);
    git(&["commit", "--no-verify", "-q", "-m", "v2"], &repo);
    git(&["tag", "v2"], &repo);

    std::fs::write(
        proj.join(".mangrove/resolvers.toml"),
        format!("[namespace.pkg]\ngit = {:?}\n", repo.to_str().unwrap()),
    )
    .unwrap();

    // Pinned to v1 (unbounded) — port 9999 is valid.
    let pinned = proj.join("pinned.mang");
    std::fs::write(
        &pinned,
        "use \"pkg/probe@v2\" as k\ntype D = { p: k.Probe @\"v1\" }\nschema D\np: { port: 9999 }\n",
    )
    .unwrap();
    assert!(
        mangrove(&["update", pinned.to_str().unwrap()])
            .status
            .success()
    );
    let ok = mangrove(&["check", pinned.to_str().unwrap()]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // Unpinned — uses v2's bounded Probe, so port 9999 is rejected.
    let unpinned = proj.join("unpinned.mang");
    std::fs::write(
        &unpinned,
        "use \"pkg/probe@v2\" as k\ntype D = { p: k.Probe }\nschema D\np: { port: 9999 }\n",
    )
    .unwrap();
    assert!(
        mangrove(&["update", unpinned.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(
        mangrove(&["check", unpinned.to_str().unwrap()])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn version_pin_on_local_import_errors() {
    let dir = std::env::temp_dir().join(format!("m6b_local_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.mang"), "type Probe = { port: int }\n").unwrap();
    let app = dir.join("app.mang");
    std::fs::write(
        &app,
        "use \"./lib.mang\" as k\ntype D = { p: k.Probe @\"v1\" }\nschema D\np: { port: 1 }\n",
    )
    .unwrap();
    // A local import has no versions to pin to → a clean error, not a crash.
    assert_eq!(
        mangrove(&["check", app.to_str().unwrap()]).status.code(),
        Some(1)
    );
}
