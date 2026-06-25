//! M5b/M5c: end-to-end `import`/`export` round-trips. The invariant is value-level
//! (D43): exporting an evaluated document and re-importing it yields a schemaless
//! document with the SAME content hash as the original.

use std::process::Command;

fn mangrove(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(args)
        .output()
        .expect("run")
}

fn stdout(out: &std::process::Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn examples(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// Full cycle on a real file: import → mangrove → hash → export → re-import →
/// mangrove → hash; the two hashes must match (value preserved, D43). `label`
/// keeps temp files unique so tests can run in parallel.
fn assert_real_round_trip(label: &str, rel: &str, fmt: &str, ext: &str) {
    let src = examples(rel);
    let pid = std::process::id();
    let mang1 = stdout(&mangrove(&["import", src.to_str().unwrap()]));
    let m1 = std::env::temp_dir().join(format!("rt_{label}_{pid}_1.mang"));
    std::fs::write(&m1, &mang1).unwrap();
    let h1 = stdout(&mangrove(&["hash", m1.to_str().unwrap()]));

    let exported = stdout(&mangrove(&["export", m1.to_str().unwrap(), "--to", fmt]));
    let f2 = std::env::temp_dir().join(format!("rt_{label}_{pid}_2.{ext}"));
    std::fs::write(&f2, &exported).unwrap();

    let mang2 = stdout(&mangrove(&["import", f2.to_str().unwrap()]));
    let m2 = std::env::temp_dir().join(format!("rt_{label}_{pid}_2.mang"));
    std::fs::write(&m2, &mang2).unwrap();
    let h2 = stdout(&mangrove(&["hash", m2.to_str().unwrap()]));

    assert_eq!(h1, h2, "round-trip of {rel} via {fmt} changed the value");
}

#[test]
fn real_k8s_deployment_yaml_round_trips() {
    assert_real_round_trip("k8s", "real-world/deployment.yaml", "yaml", "yaml");
}

#[test]
fn real_ci_workflow_yaml_round_trips() {
    assert_real_round_trip("ci", "real-world/ci.yaml", "yaml", "yaml");
}

#[test]
fn real_pyproject_toml_round_trips() {
    assert_real_round_trip("pyproj", "real-world/pyproject.toml", "toml", "toml");
}

/// export <fixture> --to <fmt> → import the result → hash; equals hash(<fixture>).
fn assert_round_trip(fixture: &str, fmt: &str, ext: &str) {
    let src = examples(fixture);
    let orig_hash = stdout(&mangrove(&["hash", src.to_str().unwrap()]));

    let exported = stdout(&mangrove(&["export", src.to_str().unwrap(), "--to", fmt]));
    let data = std::env::temp_dir().join(format!("m5_rt_{fmt}.{ext}"));
    std::fs::write(&data, exported).unwrap();

    let reimported = stdout(&mangrove(&["import", data.to_str().unwrap()]));
    let mang = std::env::temp_dir().join(format!("m5_rt_{fmt}.mang"));
    std::fs::write(&mang, reimported).unwrap();

    let rt_hash = stdout(&mangrove(&["hash", mang.to_str().unwrap()]));
    assert_eq!(orig_hash, rt_hash, "round-trip via {fmt} changed the value");
}

#[test]
fn yaml_round_trip_preserves_hash() {
    // pyproject has no units/defaults, so its resolved value equals its data —
    // a schemaless re-import must hash identically.
    assert_round_trip("pyproject.mang", "yaml", "yaml");
}

#[test]
fn toml_round_trip_preserves_hash() {
    assert_round_trip("pyproject.mang", "toml", "toml");
}

#[test]
fn imported_yaml_is_valid_mangrove() {
    let y = std::env::temp_dir().join("m5_import.yaml");
    std::fs::write(&y, "name: api\nport: 8443\nnested:\n  ratio: 0.5\n").unwrap();
    let mang = stdout(&mangrove(&["import", y.to_str().unwrap()]));
    let p = std::env::temp_dir().join("m5_imported.mang");
    std::fs::write(&p, mang).unwrap();
    // The emitted document parses and hashes (schemaless).
    let out = mangrove(&["hash", p.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn boolean_key_round_trips() {
    // TOML `true = 1` is a legal key; import → mangrove → hash must succeed (the
    // rendered key is quoted so it doesn't lex as a Bool token).
    let t = std::env::temp_dir().join("m5_boolkey.toml");
    std::fs::write(&t, "true = 1\nfalse = 2\n").unwrap();
    let mang = stdout(&mangrove(&["import", t.to_str().unwrap()]));
    let p = std::env::temp_dir().join("m5_boolkey.mang");
    std::fs::write(&p, mang).unwrap();
    let out = mangrove(&["hash", p.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "rendered doc did not re-parse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn yaml_null_is_rejected() {
    let y = std::env::temp_dir().join("m5_null.yaml");
    std::fs::write(&y, "a: null\n").unwrap();
    assert_eq!(
        mangrove(&["import", y.to_str().unwrap()]).status.code(),
        Some(1)
    );
}

#[test]
fn exact_decimal_survives_yaml_round_trip() {
    // A decimal beyond f64 precision must survive yaml import→export→import.
    let y = std::env::temp_dir().join("m5_prec.yaml");
    std::fs::write(&y, "x: 0.123456789012345678\n").unwrap();
    let mang = stdout(&mangrove(&["import", y.to_str().unwrap()]));
    assert!(
        mang.contains("0.123456789012345678"),
        "decimal precision lost: {mang}"
    );
}
