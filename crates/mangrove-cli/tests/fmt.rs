use std::process::Command;

fn mangrove(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(args)
        .output()
        .expect("run")
}

#[test]
fn fmt_rewrites_file_in_place() {
    let dir = std::env::temp_dir().join(format!("fmt_rw_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.mang");
    std::fs::write(&p, "a:{b: 1,c: 2}\n").unwrap();
    assert!(mangrove(&["fmt", p.to_str().unwrap()]).status.success());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "a: { b: 1, c: 2 }\n");
}

#[test]
fn fmt_check_exits_nonzero_when_unformatted() {
    let dir = std::env::temp_dir().join(format!("fmt_chk_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.mang");
    std::fs::write(&p, "a:1\n").unwrap();
    assert_eq!(
        mangrove(&["fmt", "--check", p.to_str().unwrap()])
            .status
            .code(),
        Some(1)
    );
    // already-formatted → exit 0, file unchanged
    std::fs::write(&p, "a: 1\n").unwrap();
    assert!(
        mangrove(&["fmt", "--check", p.to_str().unwrap()])
            .status
            .success()
    );
}

#[test]
fn fmt_stdin_to_stdout() {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(["fmt", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"a:1\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "a: 1\n");
}
