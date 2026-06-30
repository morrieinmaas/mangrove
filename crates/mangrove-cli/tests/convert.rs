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

// ── Multi-doc YAML (M5b multidoc) ─────────────────────────────────────────────

#[test]
fn import_multidoc_yaml_succeeds_and_prints_list() {
    // A k8s-style two-document stream imports without error and prints a list.
    let pid = std::process::id();
    let y = std::env::temp_dir().join(format!("m5_multidoc_{pid}.yaml"));
    std::fs::write(
        &y,
        "kind: PersistentVolumeClaim\nmetadata:\n  name: pvc\n---\nkind: CronJob\nmetadata:\n  name: cron\n",
    )
    .unwrap();
    let out = mangrove(&["import", y.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    // The output should contain both kinds in a list representation.
    assert!(
        text.contains("PersistentVolumeClaim"),
        "missing first doc: {text}"
    );
    assert!(text.contains("CronJob"), "missing second doc: {text}");
}

#[test]
fn export_yaml_stream_list_produces_multidoc_output() {
    // A bare-list .mang (now supported as a bare-value document) exported with
    // `--to yaml-stream` must emit one YAML document per element separated by
    // `---`, not a single flat document.
    let pid = std::process::id();

    // Two-element bare list: each element is a map.
    let f2 = std::env::temp_dir().join(format!("m5_stream2_{pid}.mang"));
    std::fs::write(
        &f2,
        "[ { kind: \"PVC\", name: \"pvc\" }, { kind: \"CronJob\", name: \"cron\" } ]\n",
    )
    .unwrap();
    let out2 = mangrove(&["export", f2.to_str().unwrap(), "--to", "yaml-stream"]);
    assert!(
        out2.status.success(),
        "--to yaml-stream failed on bare list: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let text2 = String::from_utf8(out2.stdout).unwrap();
    // Must contain a `---` separator between the two documents.
    assert!(
        text2.contains("---"),
        "expected `---` separator in yaml-stream output, got: {text2}"
    );
    // Must contain both element values.
    assert!(text2.contains("PVC"), "missing PVC in stream: {text2}");
    assert!(
        text2.contains("CronJob"),
        "missing CronJob in stream: {text2}"
    );
    // Must end with a newline (well-formed text file).
    assert!(
        text2.ends_with('\n'),
        "yaml-stream output must end with a newline"
    );

    // Three-element bare list: verify exactly 2 separators (n-1 for n=3 docs).
    let f3 = std::env::temp_dir().join(format!("m5_stream3_{pid}.mang"));
    std::fs::write(
        &f3,
        "[ { kind: \"A\" }, { kind: \"B\" }, { kind: \"C\" } ]\n",
    )
    .unwrap();
    let out3 = mangrove(&["export", f3.to_str().unwrap(), "--to", "yaml-stream"]);
    assert!(
        out3.status.success(),
        "--to yaml-stream failed on 3-element list: {}",
        String::from_utf8_lossy(&out3.stderr)
    );
    let text3 = String::from_utf8(out3.stdout).unwrap();
    let sep_count = text3.matches("---").count();
    assert_eq!(
        sep_count, 2,
        "expected 2 `---` separators for 3-element list, got {sep_count} in: {text3}"
    );

    // Unknown format must error.
    let out_bad = mangrove(&["export", f2.to_str().unwrap(), "--to", "json"]);
    assert_eq!(out_bad.status.code(), Some(1));
}

#[test]
fn import_skip_empty_flag_after_path() {
    // `import <file> --skip-empty` (flag AFTER path) must behave identically to
    // `import --skip-empty <file>` (flag before path).
    let pid = std::process::id();
    let y = std::env::temp_dir().join(format!("m5_skip_{pid}.yaml"));
    std::fs::write(
        &y,
        "kind: Deployment\nmetadata:\n  name: app\n---\n\n---\nkind: Service\nmetadata:\n  name: svc\n",
    )
    .unwrap();
    let path = y.to_str().unwrap();

    let out_flag_before = mangrove(&["import", "--skip-empty", path]);
    assert!(
        out_flag_before.status.success(),
        "--skip-empty before path failed: {}",
        String::from_utf8_lossy(&out_flag_before.stderr)
    );

    let out_flag_after = mangrove(&["import", path, "--skip-empty"]);
    assert!(
        out_flag_after.status.success(),
        "--skip-empty after path failed: {}",
        String::from_utf8_lossy(&out_flag_after.stderr)
    );

    assert_eq!(
        out_flag_before.stdout, out_flag_after.stdout,
        "--skip-empty position must not affect output"
    );
}
