//! `mangrove gen-openapi`: OpenAPI spec → Mangrove types → type-check manifests.

use std::process::Command;

fn mangrove(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mangrove"))
        .args(args)
        .output()
        .expect("run")
}

const SPEC: &str = r##"{ "definitions": {
  "Probe": { "type": "object", "required": ["path","port"], "properties": {
    "path": { "type": "string" }, "port": { "type": "integer" },
    "scheme": { "type": "string", "enum": ["HTTP","HTTPS"] } } },
  "Container": { "type": "object", "required": ["name"], "properties": {
    "name": { "type": "string" }, "probe": { "$ref": "#/definitions/Probe" },
    "args": { "type": "array", "items": { "type": "string" } } } }
} }"##;

fn gen_types(dir: &std::path::Path, root: &str) -> String {
    let spec = dir.join("spec.json");
    std::fs::write(&spec, SPEC).unwrap();
    let out = mangrove(&["gen-openapi", spec.to_str().unwrap(), "--root", root]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn generated_types_validate_a_conforming_manifest() {
    let dir = std::env::temp_dir().join(format!("openapi_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = gen_types(&dir, "Container");
    doc.push_str("schema Container\nname: \"api\"\nprobe: { path: \"/healthz\", port: 8443, scheme: \"HTTPS\" }\nargs: [ \"-v\" ]\n");
    let p = dir.join("app.mang");
    std::fs::write(&p, doc).unwrap();
    let out = mangrove(&["check", p.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn generated_types_reject_a_bad_manifest() {
    let dir = std::env::temp_dir().join(format!("openapi_bad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = gen_types(&dir, "Container");
    // port should be an int; a string violates the generated type
    doc.push_str("schema Container\nname: \"api\"\nprobe: { path: \"/x\", port: \"8443\" }\n");
    let p = dir.join("bad.mang");
    std::fs::write(&p, doc).unwrap();
    assert_eq!(
        mangrove(&["check", p.to_str().unwrap()]).status.code(),
        Some(1)
    );
}

#[test]
fn recursive_definition_warns_but_succeeds() {
    let dir = std::env::temp_dir().join(format!("openapi_rec_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("rec.json");
    std::fs::write(
        &spec,
        r##"{ "definitions": { "Node": { "type": "object", "required": ["v"],
          "properties": { "v": {"type":"integer"}, "next": {"$ref":"#/definitions/Node"} } } } }"##,
    )
    .unwrap();
    let out = mangrove(&["gen-openapi", spec.to_str().unwrap(), "--root", "Node"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("recursive"),
        "expected a recursion warning: {stderr}"
    );
    // the emitted types are acyclic (the self-ref was opaque'd) and parse
    let mut doc = String::from_utf8(out.stdout).unwrap();
    doc.push_str("schema Node\nv: 1\nnext: {}\n");
    let p = dir.join("node.mang");
    std::fs::write(&p, doc).unwrap();
    assert!(mangrove(&["check", p.to_str().unwrap()]).status.success());
}
