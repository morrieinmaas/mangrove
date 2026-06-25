//! M7: per-package resolver/lock anchoring (D50). A dependency resolves and
//! verifies its OWN deps against its OWN `.mangrove/resolvers.toml` + lock — a
//! namespace the root never defines, pinned by the dependency's own committed lock.

use std::process::Command;

fn mangrove(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(args)
        .output()
        .expect("run")
}

/// Build a 3-level local project: root → packageA (own resolvers) → pkgB.
/// Returns (root_dir, root.mang path, packageA lock path).
fn scaffold(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let r = std::env::temp_dir().join(format!("m7_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(r.join(".mangrove")).unwrap();
    std::fs::create_dir_all(r.join("packageA/.mangrove")).unwrap();
    std::fs::create_dir_all(r.join("packageA/pkgB")).unwrap();
    std::fs::write(r.join("packageA/pkgB/util.mang"), "name: \"fromB\"\n").unwrap();
    // packageA imports `b` via its OWN resolvers — the root never defines `b`.
    std::fs::write(
        r.join("packageA/.mangrove/resolvers.toml"),
        "[namespace.b]\nremote = \"pkgB\"\n",
    )
    .unwrap();
    std::fs::write(
        r.join("packageA/main.mang"),
        "use \"b/util@v1\" as b\n...b\n",
    )
    .unwrap();
    std::fs::write(
        r.join(".mangrove/resolvers.toml"),
        "[namespace.a]\nremote = \"packageA\"\n",
    )
    .unwrap();
    std::fs::write(r.join("root.mang"), "use \"a/main@v1\" as a\n...a\n").unwrap();
    let root_doc = r.join("root.mang");
    let a_lock = r.join("packageA/mangrove.lock");
    (r, root_doc, a_lock)
}

#[test]
fn dependency_resolves_its_own_namespaces() {
    let (_r, root, _a_lock) = scaffold("ok");
    // The dependency ships its own lock (pinning b/util@v1)…
    assert!(
        mangrove(&[
            "update",
            root.with_file_name("packageA")
                .join("main.mang")
                .to_str()
                .unwrap()
        ])
        .status
        .success()
    );
    // …the root pins only its direct dep (a/main@v1), not b.
    assert!(
        mangrove(&["update", root.to_str().unwrap()])
            .status
            .success()
    );
    let lock = std::fs::read_to_string(root.with_file_name("mangrove.lock")).unwrap();
    assert!(lock.contains("a/main@v1"), "{lock}");
    assert!(
        !lock.contains("b/util@v1"),
        "root lock must not pin the dep's dep: {lock}"
    );
    // check: packageA resolves `b` via its own resolvers + verifies via its own lock.
    let out = mangrove(&["check", root.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn dependency_lock_is_fail_closed() {
    let (_r, root, a_lock) = scaffold("fail");
    mangrove(&[
        "update",
        root.with_file_name("packageA")
            .join("main.mang")
            .to_str()
            .unwrap(),
    ]);
    mangrove(&["update", root.to_str().unwrap()]);
    // Tamper the DEPENDENCY's own lock — its dep must fail integrity at its level.
    std::fs::write(&a_lock, "\"b/util@v1\" = \"b3:deadbeef\"\n").unwrap();
    assert_eq!(
        mangrove(&["check", root.to_str().unwrap()]).status.code(),
        Some(1)
    );
}
