//! M6a: cross-file type imports — `schema k.Type` / `field: k.Type` resolve a
//! `use`d module's type definitions (including their internal refinements).

use std::process::Command;

fn scratch() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("m6_imports_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lib = "type Probe = { path: str, port: int & >= 1 & <= 65535 }\n\
               type Container = { name: str, probe: Probe }\n";
    std::fs::write(dir.join("lib.mang"), lib).unwrap();
    dir
}

fn run(cmd: &str, p: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .arg(cmd)
        .arg(p)
        .output()
        .expect("run")
}

fn hashf(p: &std::path::Path) -> String {
    let o = run("hash", p);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

#[test]
fn imported_type_as_field_type_validates() {
    let dir = scratch();
    std::fs::write(
        dir.join("app.mang"),
        "use \"./lib.mang\" as k\ntype D = { container: k.Container }\nschema D\n\
         container: { name: \"api\", probe: { path: \"/healthz\", port: 8443 } }\n",
    )
    .unwrap();
    assert!(run("check", &dir.join("app.mang")).status.success());
}

#[test]
fn imported_type_as_schema_validates() {
    let dir = scratch();
    std::fs::write(
        dir.join("app2.mang"),
        "use \"./lib.mang\" as k\nschema k.Container\nname: \"api\"\n\
         probe: { path: \"/healthz\", port: 8443 }\n",
    )
    .unwrap();
    assert!(run("check", &dir.join("app2.mang")).status.success());
}

#[test]
fn imported_types_internal_refinement_is_enforced() {
    // Probe's port range (defined in lib.mang) governs through k.Container.
    let dir = scratch();
    std::fs::write(
        dir.join("bad.mang"),
        "use \"./lib.mang\" as k\nschema k.Container\nname: \"api\"\n\
         probe: { path: \"/healthz\", port: 99999 }\n",
    )
    .unwrap();
    let out = run("check", &dir.join("bad.mang"));
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("port"));
}

#[test]
fn imported_type_hashes_like_inline() {
    // A doc using k.Container hashes identically to the same types written inline.
    let dir = scratch();
    std::fs::write(
        dir.join("via_import.mang"),
        "use \"./lib.mang\" as k\nschema k.Container\nname: \"api\"\n\
         probe: { path: \"/healthz\", port: 8443 }\n",
    )
    .unwrap();
    let inline = std::env::temp_dir().join(format!("m6_inline_{}.mang", std::process::id()));
    std::fs::write(
        &inline,
        "type Probe = { path: str, port: int & >= 1 & <= 65535 }\n\
         type Container = { name: str, probe: Probe }\nschema Container\n\
         name: \"api\"\nprobe: { path: \"/healthz\", port: 8443 }\n",
    )
    .unwrap();
    assert_eq!(hashf(&dir.join("via_import.mang")), hashf(&inline));
}
