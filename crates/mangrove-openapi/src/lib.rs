//! Generate Mangrove `type` definitions from an OpenAPI (v2 `definitions` or v3
//! `components.schemas`) spec — e.g. the Kubernetes API, so manifests can be
//! typed against the real API.
//!
//! Honest limits (Mangrove is deliberately strict):
//! - **No top type.** A free-form object (`additionalProperties: true`, or an
//!   object with neither `properties` nor a typed `additionalProperties`) cannot
//!   be represented precisely; it is modeled as the loose `OpaqueObject` (a
//!   `{ [str]: str }`) and a warning is emitted. Such fields will not accept
//!   arbitrary nested JSON.
//! - **No recursion.** A definition that (transitively) refers to itself can't
//!   exist under Mangrove's totality axiom; references that close a cycle are
//!   replaced with `OpaqueObject` and a warning is emitted.
//!
//! Target a `root` to generate just the closure you need (recommended).

use serde_json::{Map, Value as J};
use std::collections::{BTreeSet, VecDeque};

/// The generated Mangrove source plus any advisory warnings.
pub struct Generated {
    pub types: String,
    pub warnings: Vec<String>,
}

/// Generate Mangrove types for every definition reachable from `root`
/// (or all definitions if `root` is `None`).
///
/// ```
/// let spec = r#"{ "definitions": { "Port": { "type": "object",
///     "required": ["n"], "properties": { "n": { "type": "integer" } } } } }"#;
/// let g = mangrove_openapi::generate(spec, Some("Port")).unwrap();
/// assert!(g.types.contains("type Port = { n: int }"));
/// ```
pub fn generate(spec_json: &str, root: Option<&str>) -> Result<Generated, String> {
    let spec: J = serde_json::from_str(spec_json).map_err(|e| format!("invalid JSON: {e}"))?;
    let defs = definitions(&spec)
        .ok_or("spec has no `definitions` (OpenAPI v2) or `components.schemas` (v3)")?;

    let names: Vec<String> = match root {
        Some(r) => {
            if !defs.contains_key(r) {
                return Err(format!("root definition `{r}` not found in the spec"));
            }
            reachable(r, defs)
        }
        None => defs.keys().cloned().collect(),
    };
    let selected: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let in_cycle = cycle_defs(&selected, defs);

    let mut warnings = Vec::new();
    if !in_cycle.is_empty() {
        let mut ns: Vec<&str> = in_cycle.iter().copied().collect();
        ns.sort();
        warnings.push(format!(
            "{} recursive definition(s) (not representable under Mangrove's no-recursion axiom); \
             cycle-closing references modeled as `OpaqueObject`: {}",
            in_cycle.len(),
            ns.join(", ")
        ));
    }

    let mut used_opaque = false;
    let mut body = String::new();
    let mut sorted: Vec<&str> = selected.iter().copied().collect();
    sorted.sort();
    for name in sorted {
        let ty = render(
            &defs[name],
            defs,
            &in_cycle,
            &mut used_opaque,
            &mut warnings,
        );
        body.push_str(&format!("type {} = {}\n", sanitize(name), ty));
    }

    let mut types = String::new();
    if used_opaque {
        types.push_str(
            "# free-form / recursive values modeled loosely — see warnings\n\
             type OpaqueObject = { [str]: str }\n",
        );
    }
    types.push_str(&body);
    Ok(Generated { types, warnings })
}

/// The definition map, supporting both OpenAPI v2 and v3.
fn definitions(spec: &J) -> Option<&Map<String, J>> {
    spec.get("definitions").and_then(J::as_object).or_else(|| {
        spec.get("components")
            .and_then(|c| c.get("schemas"))
            .and_then(J::as_object)
    })
}

/// The definition name a `$ref` points at (its last `/`-segment).
fn ref_name(r: &str) -> &str {
    r.rsplit('/').next().unwrap_or(r)
}

/// Push every definition name referenced (transitively) by `schema`.
fn collect_refs(schema: &J, out: &mut Vec<String>) {
    match schema {
        J::Object(m) => {
            if let Some(r) = m.get("$ref").and_then(J::as_str) {
                out.push(ref_name(r).to_string());
            }
            for v in m.values() {
                collect_refs(v, out);
            }
        }
        J::Array(xs) => {
            for v in xs {
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Every definition reachable from `root` (inclusive), via `$ref`.
fn reachable(root: &str, defs: &Map<String, J>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_string());
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(schema) = defs.get(&name) {
            let mut refs = Vec::new();
            collect_refs(schema, &mut refs);
            for r in refs {
                if !seen.contains(&r) {
                    queue.push_back(r);
                }
            }
        }
    }
    seen.into_iter().collect()
}

/// The subset of `selected` definitions that participate in a `$ref` cycle.
fn cycle_defs<'a>(selected: &BTreeSet<&'a str>, defs: &Map<String, J>) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    for &name in selected {
        // A node is in a cycle iff it can reach itself.
        let mut refs = Vec::new();
        if let Some(s) = defs.get(name) {
            collect_refs(s, &mut refs);
        }
        let start: Vec<String> = refs;
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<String> = start.into_iter().collect();
        while let Some(n) = queue.pop_front() {
            if n == name {
                out.insert(name);
                break;
            }
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(s) = defs.get(&n) {
                let mut r = Vec::new();
                collect_refs(s, &mut r);
                queue.extend(r);
            }
        }
    }
    out
}

/// Sanitize an OpenAPI definition name into a valid Mangrove type name.
fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if !s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        s.insert(0, '_');
    }
    s
}

/// Render a field key: a bare identifier, else a quoted string key.
fn field_key(name: &str) -> String {
    let simple = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if simple {
        name.to_string()
    } else {
        format!("{name:?}")
    }
}

fn render(
    schema: &J,
    defs: &Map<String, J>,
    in_cycle: &BTreeSet<&str>,
    used_opaque: &mut bool,
    warnings: &mut Vec<String>,
) -> String {
    let opaque = |used: &mut bool| {
        *used = true;
        "OpaqueObject".to_string()
    };

    if let Some(r) = schema.get("$ref").and_then(J::as_str) {
        let target = ref_name(r);
        if in_cycle.contains(target) {
            return opaque(used_opaque);
        }
        return sanitize(target);
    }
    // Kubernetes int-or-string quantities.
    if schema.get("format").and_then(J::as_str) == Some("int-or-string") {
        return "int | str".to_string();
    }
    // An object with declared properties → a record.
    if schema.get("properties").and_then(J::as_object).is_some() {
        return render_record(schema, defs, in_cycle, used_opaque, warnings);
    }
    match schema.get("type").and_then(J::as_str) {
        Some("object") => match schema.get("additionalProperties") {
            Some(J::Object(_)) => {
                let v = render(
                    &schema["additionalProperties"],
                    defs,
                    in_cycle,
                    used_opaque,
                    warnings,
                );
                format!("{{ [str]: {v} }}")
            }
            _ => opaque(used_opaque),
        },
        Some("array") => match schema.get("items") {
            Some(items) => format!(
                "[ {} ]",
                render(items, defs, in_cycle, used_opaque, warnings)
            ),
            None => format!("[ {} ]", opaque(used_opaque)),
        },
        Some("string") => {
            if let Some(J::Array(en)) = schema.get("enum") {
                let lits: Vec<String> = en
                    .iter()
                    .filter_map(J::as_str)
                    .map(|s| format!("{s:?}"))
                    .collect();
                if !lits.is_empty() {
                    return lits.join(" | ");
                }
            }
            "str".to_string()
        }
        Some("integer") => "int".to_string(),
        Some("number") => "decimal".to_string(),
        Some("boolean") => "bool".to_string(),
        _ => opaque(used_opaque),
    }
}

fn render_record(
    schema: &J,
    defs: &Map<String, J>,
    in_cycle: &BTreeSet<&str>,
    used_opaque: &mut bool,
    warnings: &mut Vec<String>,
) -> String {
    let props = schema["properties"].as_object().unwrap();
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(J::as_array)
        .map(|a| a.iter().filter_map(J::as_str).collect())
        .unwrap_or_default();
    let mut fields = Vec::new();
    for (pname, pschema) in props {
        let opt = if required.contains(pname.as_str()) {
            ""
        } else {
            "?"
        };
        let ty = render(pschema, defs, in_cycle, used_opaque, warnings);
        fields.push(format!("{}{opt}: {ty}", field_key(pname)));
    }
    if fields.is_empty() {
        // an object typed only by `properties: {}` — treat as opaque
        *used_opaque = true;
        return "OpaqueObject".to_string();
    }
    format!("{{ {} }}", fields.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r##"{
      "definitions": {
        "Probe": {
          "type": "object",
          "required": ["path", "port"],
          "properties": {
            "path": { "type": "string" },
            "port": { "type": "integer" },
            "scheme": { "type": "string", "enum": ["HTTP", "HTTPS"] }
          }
        },
        "Container": {
          "type": "object",
          "required": ["name"],
          "properties": {
            "name": { "type": "string" },
            "probe": { "$ref": "#/definitions/Probe" },
            "args": { "type": "array", "items": { "type": "string" } },
            "labels": { "type": "object", "additionalProperties": { "type": "string" } },
            "ratio": { "type": "number" }
          }
        }
      }
    }"##;

    #[test]
    fn generates_records_with_required_optional_enum_ref_array_map() {
        let g = generate(SPEC, Some("Container")).unwrap();
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        // both reachable defs emitted
        assert!(g.types.contains("type Container ="));
        assert!(g.types.contains("type Probe ="));
        // required vs optional, ref, array, map, enum, decimal
        assert!(g.types.contains("name: str"));
        assert!(g.types.contains("probe?: Probe"));
        assert!(g.types.contains("args?: [ str ]"));
        assert!(g.types.contains("labels?: { [str]: str }"));
        assert!(g.types.contains("ratio?: decimal"));
        assert!(g.types.contains("scheme?: \"HTTP\" | \"HTTPS\""));
    }

    #[test]
    fn root_closure_excludes_unreferenced_defs() {
        // Probe alone doesn't pull in Container.
        let g = generate(SPEC, Some("Probe")).unwrap();
        assert!(g.types.contains("type Probe ="));
        assert!(!g.types.contains("type Container ="));
    }

    #[test]
    fn recursion_is_broken_to_opaque_with_warning() {
        let spec = r##"{ "definitions": {
          "Node": { "type": "object", "required": ["v"], "properties": {
            "v": { "type": "integer" },
            "next": { "$ref": "#/definitions/Node" }
          }}
        }}"##;
        let g = generate(spec, Some("Node")).unwrap();
        assert_eq!(g.warnings.len(), 1);
        assert!(g.warnings[0].contains("recursive"));
        assert!(g.types.contains("OpaqueObject")); // the self-ref was broken
        assert!(g.types.contains("type Node ="));
    }

    #[test]
    fn sanitizes_dotted_kubernetes_names() {
        let spec = r##"{ "definitions": {
          "io.k8s.api.apps.v1.Deployment": { "type": "object", "required": ["x"],
            "properties": { "x": { "type": "integer" } } }
        }}"##;
        let g = generate(spec, None).unwrap();
        assert!(g.types.contains("type io_k8s_api_apps_v1_Deployment ="));
    }

    #[test]
    fn unknown_root_errors() {
        assert!(generate(SPEC, Some("Nope")).is_err());
    }
}
