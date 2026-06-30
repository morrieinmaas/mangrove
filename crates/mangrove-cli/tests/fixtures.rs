//! M5a: end-to-end checks of the real-world example documents. These exercise
//! the whole pipeline (types + refinements + units + lists + composition) on
//! realistic config, and pin a golden content hash so an unintended change to
//! canonicalization is caught. Regenerate the goldens only on an intentional change.

use std::process::Command;

fn examples_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn run(cmd: &str, path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg(cmd)
        .arg(path)
        .output()
        .expect("run")
}

#[test]
fn k8s_deployment_checks_and_hash_is_stable() {
    let p = examples_dir().join("k8s-deployment.mang");
    let chk = run("check", &p);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
    let hash = run("hash", &p);
    assert_eq!(
        String::from_utf8(hash.stdout).unwrap().trim(),
        "b3:07b4520655d7530f25c44c100326a99de01ce2dc4218c6dec80ca943b4797924"
    );
}

#[test]
fn pyproject_checks_and_hash_is_stable() {
    let p = examples_dir().join("pyproject.mang");
    let chk = run("check", &p);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
    let hash = run("hash", &p);
    assert_eq!(
        String::from_utf8(hash.stdout).unwrap().trim(),
        "b3:a28a356acde16026359eac1171c9e37587f8021bc531d63820345c18f7c2a212"
    );
}

#[test]
fn templated_k8s_checks_and_hash_is_stable() {
    // L3 showcase: params + match (per-env replicas) + interpolation + units.
    let p = examples_dir().join("k8s-templated.mang");
    let chk = run("check", &p);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
    let hash = run("hash", &p);
    assert_eq!(
        String::from_utf8(hash.stdout).unwrap().trim(),
        "b3:6cf957239dcb33de53ce4ea0dcdbc2a03059920b41d2def0745bec53e3a35644"
    );
}

/// Run `mangrove <subcmd> <path> [extra_args…]` and return the full `Output`.
fn run_extra(subcmd: &str, path: &std::path::Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg(subcmd)
        .arg(path)
        .args(extra)
        .output()
        .expect("run")
}

#[test]
fn gitops_resources_checks_and_hash_is_stable() {
    // v0.10.x/v0.11.0 conformance: DU + len refinement + bare-list body +
    // conditional element + list spread, all exercised together.
    let p = examples_dir().join("gitops-resources.mang");

    // 1. `check` must succeed with default params.
    let chk = run("check", &p);
    assert!(
        chk.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&chk.stderr)
    );

    // 2. `hash` must equal the pinned golden.
    let hash = run("hash", &p);
    assert_eq!(
        String::from_utf8(hash.stdout).unwrap().trim(),
        "b3:94af384b06da893f4be84c2e4a21ec9a3e0c703eee894d58caa2d25163689269"
    );

    // 3. yaml-stream export produces exactly 4 resources (3 `---` separators).
    let export = run_extra("export", &p, &["--to", "yaml-stream"]);
    assert!(
        export.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    let stream = String::from_utf8(export.stdout).unwrap();
    let sep_count = stream.matches("---").count();
    assert_eq!(
        sep_count, 3,
        "expected 3 `---` separators for 4 resources, got {sep_count} in:\n{stream}"
    );

    // 4. Round-trip: export → re-import → hash must equal the original.
    let rt_yaml = std::env::temp_dir().join("gitops_rt.yaml");
    std::fs::write(&rt_yaml, &stream).unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(["import", "--skip-empty"])
        .arg(&rt_yaml)
        .output()
        .expect("import");
    assert!(
        imported.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let rt_mang = std::env::temp_dir().join("gitops_rt.mang");
    std::fs::write(&rt_mang, imported.stdout).unwrap();
    let rt_hash = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("hash")
        .arg(&rt_mang)
        .output()
        .expect("hash");
    assert!(
        rt_hash.status.success(),
        "hash after round-trip failed: {}",
        String::from_utf8_lossy(&rt_hash.stderr)
    );
    assert_eq!(
        String::from_utf8(rt_hash.stdout).unwrap().trim(),
        "b3:94af384b06da893f4be84c2e4a21ec9a3e0c703eee894d58caa2d25163689269",
        "yaml-stream round-trip must be content-hash-stable"
    );
}

#[test]
fn gitops_resources_too_long_name_fails() {
    // Negative: a name exceeding 63 chars violates the `len <= 63` refinement on
    // `Name`. The error must name the violating path and the refinement.
    let src = std::fs::read_to_string(examples_dir().join("gitops-resources.mang")).unwrap();
    let bad = src.replace(
        r#""data-pvc""#,
        r#""this-name-is-way-too-long-to-be-a-valid-kubernetes-resource-name-abcdef""#,
    );
    let p = std::env::temp_dir().join("gitops_bad_len.mang");
    std::fs::write(&p, bad).unwrap();
    let out = run("check", &p);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 for too-long name"
    );
    // The CLI writes validation errors to stdout (not stderr).
    let output = String::from_utf8_lossy(&out.stdout);
    assert!(
        output.contains("len <= 63"),
        "output must mention the violated refinement `len <= 63`, got:\n{output}"
    );
    assert!(
        output.contains("metadata.name"),
        "output must contain the path `metadata.name`, got:\n{output}"
    );
}

#[test]
fn gitops_resources_unknown_kind_fails() {
    // Negative: a `kind: "Deployment"` is not in the DU — the error must name
    // the unknown discriminant and list the valid kinds.
    // We replace the PVC *body element* (which includes its spec line) without
    // touching the type definition, by matching the unique multi-field sequence.
    let src = std::fs::read_to_string(examples_dir().join("gitops-resources.mang")).unwrap();
    let bad = src.replace(
        "kind: \"PersistentVolumeClaim\"\n    metadata: { name: \"data-pvc\" }\n    spec: { storageClassName: \"standard\", storage: \"10Gi\" }",
        "kind: \"Deployment\"\n    metadata: { name: \"data-pvc\" }\n    spec: { replicas: 3 }",
    );
    let p = std::env::temp_dir().join("gitops_bad_kind.mang");
    std::fs::write(&p, bad).unwrap();
    let out = run("check", &p);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 for unknown kind"
    );
    // The CLI writes validation errors to stdout (not stderr).
    let output = String::from_utf8_lossy(&out.stdout);
    assert!(
        output.contains("Deployment"),
        "output must mention the unknown kind `Deployment`, got:\n{output}"
    );
    assert!(
        output.contains("PersistentVolumeClaim") || output.contains("CronJob"),
        "output must list valid kind values, got:\n{output}"
    );
}

#[test]
fn k8s_out_of_range_replicas_fails() {
    // The replicas refinement (1..=100) is enforced on the real fixture.
    let src = std::fs::read_to_string(examples_dir().join("k8s-deployment.mang")).unwrap();
    let bad = src.replace("replicas: 3", "replicas: 999");
    let p = std::env::temp_dir().join("m5_k8s_bad.mang");
    std::fs::write(&p, bad).unwrap();
    assert_eq!(run("check", &p).status.code(), Some(1));
}

#[test]
fn pyproject_malformed_version_fails() {
    let src = std::fs::read_to_string(examples_dir().join("pyproject.mang")).unwrap();
    let bad = src.replace("version: \"1.21.0\"", "version: \"not-a-version\"");
    let p = std::env::temp_dir().join("m5_pyproject_bad.mang");
    std::fs::write(&p, bad).unwrap();
    assert_eq!(run("check", &p).status.code(), Some(1));
}
