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
fn omitted_default_hashes_same_as_explicit() {
    let schema = "type D = { name: str, replicas: int | *1 }\nschema D\n";
    let omitted = std::env::temp_dir().join("m2c_omit.mang");
    let explicit = std::env::temp_dir().join("m2c_expl.mang");
    std::fs::write(&omitted, format!("{schema}name: \"a\"\n")).unwrap();
    std::fs::write(&explicit, format!("{schema}name: \"a\"\nreplicas: 1\n")).unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&omitted), h(&explicit)); // §7 step 3 / D18
}

#[test]
fn composed_overlay_hashes_like_handwritten() {
    let dir = std::env::temp_dir().join(format!("m3a_cli_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("base.mang"),
        "name: \"api\"\nport: 8080\nenv: \"dev\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("over.mang"),
        "use \"./base.mang\" as base\n...base\nport: 9090\nenv: unset\n",
    )
    .unwrap();
    // hand-written equivalent of the composed result: name + port(9090), no env
    let hand = dir.join("hand.mang");
    std::fs::write(&hand, "name: \"api\"\nport: 9090\n").unwrap();

    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&dir.join("over.mang")), h(&hand)); // D12: compose ⇒ same value ⇒ same hash
}

#[test]
fn subtype_redefinition_accepts_narrowing_rejects_loosening() {
    let check = |name: &str, body: &str| {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, body).unwrap();
        Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("check")
            .arg(&p)
            .output()
            .unwrap()
    };
    // accept: narrow `replicas: int` to a bounded subrange; value in range → ok
    let ok = check(
        "m3a_sub_ok.mang",
        "type Dep = { replicas: int }\nschema Dep & { replicas: int & >= 1 & <= 10 }\nreplicas: 5\n",
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    // the narrowed bound is enforced: 50 > 10 → invalid
    let oob = check(
        "m3a_sub_oob.mang",
        "type Dep = { replicas: int }\nschema Dep & { replicas: int & >= 1 & <= 10 }\nreplicas: 50\n",
    );
    assert_eq!(oob.status.code(), Some(1));
    // reject: loosening (int is wider than the base's bounded int) → load error
    let loosen = check(
        "m3a_sub_loosen.mang",
        "type Dep = { replicas: int & <= 10 }\nschema Dep & { replicas: int }\nreplicas: 5\n",
    );
    assert_eq!(loosen.status.code(), Some(1));
}

#[test]
fn update_writes_lockfile_then_check_passes() {
    let dir = std::env::temp_dir().join(format!("m3b_cli_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::fs::create_dir_all(dir.join(".mangrove")).unwrap();
    std::fs::write(dir.join("vendor/base.mang"), "name: \"shared\"\n").unwrap();
    std::fs::write(
        dir.join(".mangrove/resolvers.toml"),
        "[namespace.infra]\nremote = \"vendor\"\n",
    )
    .unwrap();
    let root = dir.join("root.mang");
    std::fs::write(&root, "use \"infra/base@v1\" as k\n...k\n").unwrap();

    let run = |cmd: &str| {
        Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg(cmd)
            .arg(&root)
            .output()
            .unwrap()
    };
    // no lockfile yet → check fails closed
    assert_eq!(run("check").status.code(), Some(1));
    // update writes the lock
    let upd = run("update");
    assert!(
        upd.status.success(),
        "{}",
        String::from_utf8_lossy(&upd.stderr)
    );
    assert!(dir.join("mangrove.lock").exists());
    // now check passes (verified import)
    let chk = run("check");
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
}

#[test]
fn param_default_reference_hashes_like_literal() {
    // D12/D35: a templated doc evaluates to the same value as the hand-written one.
    let schema = "type D = { replicas: int }\nschema D\n";
    let templ = std::env::temp_dir().join("m4a_templ.mang");
    let lit = std::env::temp_dir().join("m4a_lit.mang");
    std::fs::write(
        &templ,
        format!("params {{ n: int = 3 }}\n{schema}replicas: n\n"),
    )
    .unwrap();
    std::fs::write(&lit, format!("{schema}replicas: 3\n")).unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&templ), h(&lit));
}

#[test]
fn scale_equivalent_decimal_interpolations_hash_equal() {
    // D12: 1.0 and 1.00 are the same canonical decimal, so interpolating either
    // must yield the same content address (regression for the render_scalar fix).
    let schema = "type D = { s: str }\nschema D\n";
    let a = std::env::temp_dir().join("m4_dec_a.mang");
    let b = std::env::temp_dir().join("m4_dec_b.mang");
    std::fs::write(
        &a,
        format!("params {{ v: decimal = 1.0 }}\n{schema}s: \"${{v}}\"\n"),
    )
    .unwrap();
    std::fs::write(
        &b,
        format!("params {{ v: decimal = 1.00 }}\n{schema}s: \"${{v}}\"\n"),
    )
    .unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&a), h(&b));
}

#[test]
fn fn_call_hashes_like_literal() {
    // §6.2: port(8443) is sugar for { number: 8443, name: "http" }.
    let types = "type Port = { number: int, name: str }\ntype D = { p: Port }\n";
    let templ = std::env::temp_dir().join("m4d_templ.mang");
    let lit = std::env::temp_dir().join("m4d_lit.mang");
    std::fs::write(
        &templ,
        format!(
            "{types}fn port(n: int): Port = {{ number: n, name: \"http\" }}\nschema D\np: port(8443)\n"
        ),
    )
    .unwrap();
    std::fs::write(
        &lit,
        format!("{types}schema D\np: {{ number: 8443, name: \"http\" }}\n"),
    )
    .unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&templ), h(&lit));
}

#[test]
fn match_hashes_like_literal() {
    // §6.1: replicas: match env { dev: 1, staging: 2, prod: 6 } with env="prod".
    let schema = "type D = { replicas: int }\nschema D\n";
    let templ = std::env::temp_dir().join("m4c_templ.mang");
    let lit = std::env::temp_dir().join("m4c_lit.mang");
    std::fs::write(
        &templ,
        format!(
            "params {{ env: \"dev\" | \"staging\" | \"prod\" = \"prod\" }}\n{schema}\
             replicas: match env {{ dev: 1, staging: 2, prod: 6 }}\n"
        ),
    )
    .unwrap();
    std::fs::write(&lit, format!("{schema}replicas: 6\n")).unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&templ), h(&lit));
}

#[test]
fn nonexhaustive_match_fails_check() {
    // No `_` and the scrutinee union isn't fully covered → eval error (D37).
    let out = run_check(
        "m4c_nonex.mang",
        "params { env: \"dev\" | \"prod\" = \"dev\" }\ntype D = { r: int }\nschema D\nr: match env { dev: 1 }\n",
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn interpolation_hashes_like_literal() {
    // §6.3: "api:${v}" with v="1.0" evaluates to the same value as "api:1.0".
    let schema = "type D = { image: str }\nschema D\n";
    let templ = std::env::temp_dir().join("m4b_templ.mang");
    let lit = std::env::temp_dir().join("m4b_lit.mang");
    std::fs::write(
        &templ,
        format!("params {{ v: str = \"1.0\" }}\n{schema}image: \"api:${{v}}\"\n"),
    )
    .unwrap();
    std::fs::write(&lit, format!("{schema}image: \"api:1.0\"\n")).unwrap();
    let h = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_mangrove"))
            .arg("hash")
            .arg(p)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(h(&templ), h(&lit));
}

#[test]
fn required_param_without_value_fails_check() {
    // A doc with an unbound required param is a function, not a value (D34).
    let out = run_check("m4a_req.mang", "params { v: str }\na: 1\n");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("v"));
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
