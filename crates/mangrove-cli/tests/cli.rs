use std::process::Command;

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
