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
fn generated_range_and_pattern_refinements_reject_out_of_bounds_values() {
    // The whole "catch a bullshit value" story: a spec with a numeric range and a
    // string pattern → generated refinements → `check` enforces them end to end.
    let dir = std::env::temp_dir().join(format!("openapi_refine_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("refine.json");
    std::fs::write(
        &spec,
        r##"{ "definitions": { "Port": { "type": "object", "required": ["num","name"],
          "properties": {
            "num":  { "type": "integer", "minimum": 1, "maximum": 65535 },
            "name": { "type": "string", "pattern": "[a-z]+" } } } } }"##,
    )
    .unwrap();
    let out = mangrove(&["gen-openapi", spec.to_str().unwrap(), "--root", "Port"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let types = String::from_utf8(out.stdout).unwrap();
    assert!(types.contains("num: int & >= 1 & <= 65535"), "{types}");
    assert!(types.contains(r#"name: str & =~ "[a-z]+""#), "{types}");

    // conforming value passes
    let ok = format!("{types}schema Port\nnum: 8443\nname: \"https\"\n");
    let pok = dir.join("ok.mang");
    std::fs::write(&pok, ok).unwrap();
    assert!(
        mangrove(&["check", pok.to_str().unwrap()]).status.success(),
        "conforming manifest should pass"
    );

    // out-of-range port is rejected
    let bad = format!("{types}schema Port\nnum: 99999\nname: \"https\"\n");
    let pbad = dir.join("bad.mang");
    std::fs::write(&pbad, bad).unwrap();
    assert_eq!(
        mangrove(&["check", pbad.to_str().unwrap()]).status.code(),
        Some(1),
        "out-of-range port should be rejected"
    );
}

#[test]
fn free_form_object_becomes_json_and_validates_nested() {
    // `additionalProperties: true` → a `Json` field that accepts arbitrary nesting (M8).
    let dir = std::env::temp_dir().join(format!("openapi_ff_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("ff.json");
    std::fs::write(
        &spec,
        r##"{ "definitions": { "M": { "type": "object", "required": ["data"],
          "properties": { "data": { "type": "object", "additionalProperties": true } } } } }"##,
    )
    .unwrap();
    let out = mangrove(&["gen-openapi", spec.to_str().unwrap(), "--root", "M"]);
    assert!(out.status.success());
    let mut doc = String::from_utf8(out.stdout).unwrap();
    assert!(doc.contains("data: Json"), "{doc}");
    assert!(doc.contains("type Json ="), "{doc}");
    doc.push_str("schema M\ndata: { a: 1, b: [ true, \"x\", { c: 2 } ] }\n");
    let p = dir.join("m.mang");
    std::fs::write(&p, doc).unwrap();
    let chk = mangrove(&["check", p.to_str().unwrap()]);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
}

#[test]
fn recursive_definition_emitted_faithfully_and_validates_nesting() {
    // `next` under `properties` is *productive* recursion (M8) — emitted faithfully
    // (no warning); a nested value validates against the recursive type.
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
    assert!(stderr.is_empty(), "expected no warning, got: {stderr}");
    let mut doc = String::from_utf8(out.stdout).unwrap();
    // a 3-deep recursive value validates against the recursive type
    doc.push_str("schema Node\nv: 1\nnext: { v: 2, next: { v: 3 } }\n");
    let p = dir.join("node.mang");
    std::fs::write(&p, doc).unwrap();
    let chk = mangrove(&["check", p.to_str().unwrap()]);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stderr)
    );
}
