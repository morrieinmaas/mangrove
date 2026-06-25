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
