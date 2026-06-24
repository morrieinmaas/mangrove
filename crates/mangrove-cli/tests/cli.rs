use std::process::Command;

fn run_check(name: &str, contents: &str) -> std::process::Output {
    let p = std::env::temp_dir().join(name);
    std::fs::write(&p, contents).unwrap();
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("check")
        .arg(&p)
        .output()
        .expect("run")
}

#[test]
fn check_valid_document_exits_0() {
    let out = run_check(
        "m2a_ok.mang",
        concat!(
            "type Server = { host: str, port: int & >= 1 & <= 65535 }\n",
            "schema Server\n",
            "host: \"h\"\nport: 8443\n"
        ),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_invalid_document_exits_1_and_names_field() {
    let out = run_check(
        "m2a_bad.mang",
        concat!(
            "type Server = { host: str, port: int & >= 1 & <= 65535 }\n",
            "schema Server\n",
            "host: \"h\"\nport: 70000\n"
        ),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("port"));
}

#[test]
fn check_no_schema_is_ok() {
    let out = run_check("m2a_noschema.mang", "a: 1\n");
    assert!(out.status.success());
}

#[test]
fn hash_resolves_units_so_512mi_equals_536870912() {
    let unit =
        "unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki }\ntype D = { size: Bytes }\nschema D\n";
    let a = std::env::temp_dir().join("m2b_a.mang");
    let b = std::env::temp_dir().join("m2b_b.mang");
    std::fs::write(&a, format!("{unit}size: 512Mi\n")).unwrap();
    std::fs::write(&b, format!("{unit}size: 536870912\n")).unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&a), h(&b)); // §4.5: 512Mi and 536870912 are the same value
}

#[test]
fn schemaless_unit_literal_errors() {
    let p = std::env::temp_dir().join("m2b_bare.mang");
    std::fs::write(&p, "x: 512Mi\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("hash")
        .arg(&p)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn version_flag_prints_name_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("--version")
        .output()
        .expect("failed to run mangrove");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(stdout.starts_with("mangrove "), "stdout was {stdout:?}");
}

#[test]
fn hash_subcommand_prints_b3() {
    let path = std::env::temp_dir().join("m1_cli_smoke.mang");
    std::fs::write(&path, "name: \"smoke\"\nreplicas: 1\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("hash")
        .arg(&path)
        .output()
        .expect("run");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(
        s.starts_with("b3:") && s.trim().len() == 3 + 64,
        "got {s:?}"
    );
}

#[test]
fn hash_of_invalid_file_exits_1() {
    let path = std::env::temp_dir().join("m1_cli_bad.mang");
    std::fs::write(&path, "a: ").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("hash")
        .arg(&path)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn unknown_args_exit_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg("frobnicate")
        .output()
        .expect("failed to run mangrove");
    assert_eq!(out.status.code(), Some(2));
}
