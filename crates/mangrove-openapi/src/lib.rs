//! Generate Mangrove `type` definitions from an OpenAPI (v2 `definitions` or v3
//! `components.schemas`) spec — e.g. the Kubernetes API, so manifests can be
//! typed against the real API.
//!
//! Notes (Mangrove is deliberately strict):
//! - **Free-form objects** (`additionalProperties: true`, or an object with no
//!   `properties` and no typed `additionalProperties`) map to a recursive
//!   `Json = str | int | decimal | bool | [Json] | { [str]: Json }` (M8). This
//!   accepts arbitrary nested JSON — except `null`, which Mangrove has no value
//!   for (§2.4), so a free-form value containing JSON null is rejected.
//! - **Recursive schemas** (e.g. CRD `JSONSchemaProps`) are emitted faithfully
//!   when the recursion is *productive* (guarded by a record/list/map — which it
//!   essentially always is in OpenAPI, via `properties`/`items`). A rare
//!   *non-productive* cycle is modeled as `Json` with a warning.
//!
//! Target a `root` to generate just the closure you need (recommended).

use serde_json::{Map, Value as J};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
            "{} non-productively-recursive definition(s) (recursion not guarded by a \
             record/list/map, so not representable); modeled as `Json`: {}",
            in_cycle.len(),
            ns.join(", ")
        ));
    }

    // Only definitions actually present in the spec are emitted; a dangling
    // `$ref` resolves to no emitted type (→ opaque), never an index panic.
    let mut present: Vec<&str> = selected
        .iter()
        .copied()
        .filter(|n| defs.contains_key(*n))
        .collect();
    present.sort();

    // Assign a unique, collision-resolved Mangrove name to each emitted def, so
    // two definitions that sanitize to the same identifier don't both emit
    // `type X = …` (which `TypeEnv` would reject as a duplicate). `$ref`s resolve
    // through this same map, so references stay consistent.
    let mut type_names: BTreeMap<String, String> = BTreeMap::new();
    let mut used: BTreeSet<String> = ["Json".to_string()].into_iter().collect();
    for orig in &present {
        let base = sanitize(orig);
        let mut n = base.clone();
        let mut i = 2;
        while used.contains(&n) {
            n = format!("{base}_{i}");
            i += 1;
        }
        used.insert(n.clone());
        type_names.insert((*orig).to_string(), n);
    }

    let mut used_opaque = false;
    let mut body = String::new();
    for orig in &present {
        let ty = render(
            &defs[*orig],
            defs,
            &in_cycle,
            &type_names,
            &mut used_opaque,
            &mut warnings,
        );
        body.push_str(&format!("type {} = {}\n", type_names[*orig], ty));
    }

    let mut types = String::new();
    if used_opaque {
        // Arbitrary JSON, as a productive recursive type (M8). No `null` member —
        // Mangrove has no null (§2.4), so a free-form value containing JSON null
        // is rejected, consistent with the language.
        types.push_str("type Json = str | int | decimal | bool | [ Json ] | { [str]: Json }\n");
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

/// The *unguarded* references of a schema — those reachable without descending
/// into a value-consuming position (`properties`/`items`/`additionalProperties`).
/// `$ref` directly, or under `allOf`/`oneOf`/`anyOf`, is unguarded; mirrors
/// Mangrove's productivity rule (M8) so only non-productive cycles are broken.
fn collect_unguarded_refs(schema: &J, out: &mut Vec<String>) {
    if let J::Object(m) = schema {
        if let Some(r) = m.get("$ref").and_then(J::as_str) {
            out.push(ref_name(r).to_string());
        }
        for key in ["allOf", "oneOf", "anyOf"] {
            if let Some(J::Array(arr)) = m.get(key) {
                for s in arr {
                    collect_unguarded_refs(s, out);
                }
            }
        }
        // properties / items / additionalProperties are guarded — not descended.
    }
}

/// The subset of `selected` definitions in a *non-productive* `$ref` cycle (one
/// reachable without crossing a record/list/map). Productive recursion is
/// representable in Mangrove (M8) and emitted faithfully; only these break.
fn cycle_defs<'a>(selected: &BTreeSet<&'a str>, defs: &Map<String, J>) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    for &name in selected {
        // A node is non-productively recursive iff it reaches itself via unguarded refs.
        let mut refs = Vec::new();
        if let Some(s) = defs.get(name) {
            collect_unguarded_refs(s, &mut refs);
        }
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<String> = refs.into_iter().collect();
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
                collect_unguarded_refs(s, &mut r);
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

/// Emit a Mangrove string literal. Unlike Rust's `{:?}`, this also escapes `$`,
/// which Mangrove interpolates inside `"…"` — so e.g. a property named `$ref` or
/// an enum value `${HOME}` round-trips as a literal instead of a parse error.
fn mstr(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '$' => out.push_str("\\$"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render a field key: a bare identifier, else a quoted (Mangrove-escaped) key.
fn field_key(name: &str) -> String {
    let simple = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if simple { name.to_string() } else { mstr(name) }
}

fn render(
    schema: &J,
    defs: &Map<String, J>,
    in_cycle: &BTreeSet<&str>,
    type_names: &BTreeMap<String, String>,
    used_opaque: &mut bool,
    warnings: &mut Vec<String>,
) -> String {
    let opaque = |used: &mut bool| {
        *used = true;
        "Json".to_string()
    };

    if let Some(r) = schema.get("$ref").and_then(J::as_str) {
        let target = ref_name(r);
        // Resolve through the emitted-name map; a cycle-closing or dangling ref
        // becomes opaque (so the output is always acyclic and builds).
        return match type_names.get(target) {
            Some(n) if !in_cycle.contains(target) => n.clone(),
            _ => opaque(used_opaque),
        };
    }
    // `allOf` is an intersection: representable only when it wraps a single schema
    // (the common k8s `allOf: [{$ref}]` + description). Multiple → opaque.
    if let Some(J::Array(arr)) = schema.get("allOf") {
        return match arr.as_slice() {
            [one] => render(one, defs, in_cycle, type_names, used_opaque, warnings),
            _ => opaque(used_opaque),
        };
    }
    // `oneOf`/`anyOf` → a Mangrove union of the (non-null) members.
    for key in ["oneOf", "anyOf"] {
        if let Some(J::Array(arr)) = schema.get(key) {
            let mut seen = BTreeSet::new();
            let parts: Vec<String> = arr
                .iter()
                .filter(|m| m.get("type").and_then(J::as_str) != Some("null"))
                .map(|m| render(m, defs, in_cycle, type_names, used_opaque, warnings))
                .filter(|p| seen.insert(p.clone()))
                .collect();
            return if parts.is_empty() {
                opaque(used_opaque)
            } else {
                parts.join(" | ")
            };
        }
    }
    // Kubernetes int-or-string quantities.
    if schema.get("format").and_then(J::as_str) == Some("int-or-string") {
        return "int | str".to_string();
    }
    // An object with declared properties → a record.
    if schema.get("properties").and_then(J::as_object).is_some() {
        return render_record(schema, defs, in_cycle, type_names, used_opaque, warnings);
    }
    // `type` may be a string or, in OpenAPI v3, an array like ["string","null"].
    // Mangrove has no null, so the non-null member governs (nullability is carried
    // by the field's optionality, not a null value).
    let type_str: Option<String> = match schema.get("type") {
        Some(J::String(s)) => Some(s.clone()),
        Some(J::Array(a)) => a
            .iter()
            .filter_map(J::as_str)
            .find(|s| *s != "null")
            .map(String::from),
        _ => None,
    };
    match type_str.as_deref() {
        Some("object") => match schema.get("additionalProperties") {
            Some(J::Object(_)) => {
                let v = render(
                    &schema["additionalProperties"],
                    defs,
                    in_cycle,
                    type_names,
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
                render(items, defs, in_cycle, type_names, used_opaque, warnings)
            ),
            None => format!("[ {} ]", opaque(used_opaque)),
        },
        Some("string") => {
            if let Some(J::Array(en)) = schema.get("enum") {
                let lits: Vec<String> = en.iter().filter_map(J::as_str).map(mstr).collect();
                if !lits.is_empty() {
                    return lits.join(" | ");
                }
            }
            if schema.get("minLength").is_some() || schema.get("maxLength").is_some() {
                warnings.push(
                    "string minLength/maxLength is not expressible as a Mangrove type \
                     refinement; ignored"
                        .into(),
                );
            }
            match schema.get("pattern").and_then(J::as_str) {
                Some(p) => format!("str & =~ {}", mstr(p)),
                None => "str".to_string(),
            }
        }
        Some("integer") => format!("int{}", numeric_bounds(schema)),
        Some("number") => format!("decimal{}", numeric_bounds(schema)),
        Some("boolean") => "bool".to_string(),
        _ => opaque(used_opaque),
    }
}

/// Inclusive numeric bounds → a Mangrove range refinement suffix, e.g. ` & >= 1 & <= 100`.
/// Open-ended on either side. Exclusive bounds (`exclusiveMinimum`/`Maximum`) are not
/// expressible as inclusive ranges and are ignored.
fn numeric_bounds(schema: &J) -> String {
    let mut out = String::new();
    if let Some(J::Number(n)) = schema.get("minimum") {
        out.push_str(&format!(" & >= {n}"));
    }
    if let Some(J::Number(n)) = schema.get("maximum") {
        out.push_str(&format!(" & <= {n}"));
    }
    out
}

fn render_record(
    schema: &J,
    defs: &Map<String, J>,
    in_cycle: &BTreeSet<&str>,
    type_names: &BTreeMap<String, String>,
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
        let ty = render(pschema, defs, in_cycle, type_names, used_opaque, warnings);
        fields.push(format!("{}{opt}: {ty}", field_key(pname)));
    }
    if fields.is_empty() {
        // an object typed only by `properties: {}` — arbitrary JSON
        *used_opaque = true;
        return "Json".to_string();
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
    fn productive_recursive_schema_emitted_faithfully() {
        // `next` is under `properties` (guarded), so the recursion is productive —
        // emitted faithfully, no warning, no `Json` fallback (M8).
        let spec = r##"{ "definitions": {
          "Node": { "type": "object", "required": ["v"], "properties": {
            "v": { "type": "integer" },
            "next": { "$ref": "#/definitions/Node" }
          }}
        }}"##;
        let g = generate(spec, Some("Node")).unwrap();
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        assert!(g.types.contains("next?: Node"), "{}", g.types);
        assert!(!g.types.contains("Json"), "{}", g.types);
    }

    #[test]
    fn non_productive_recursion_uses_json_with_warning() {
        // A top-level `allOf` self-reference is unguarded → non-productive → `Json`.
        let spec = r##"{ "definitions": {
          "T": { "allOf": [ { "$ref": "#/definitions/T" } ] }
        }}"##;
        let g = generate(spec, Some("T")).unwrap();
        assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
        assert!(g.types.contains("type T = Json"), "{}", g.types);
    }

    #[test]
    fn integer_minimum_maximum_become_a_range_refinement() {
        let spec = r##"{ "definitions": { "S": { "type": "object", "required": ["port"],
          "properties": { "port": { "type": "integer", "minimum": 1, "maximum": 65535 } } } } }"##;
        let g = generate(spec, Some("S")).unwrap();
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        assert!(
            g.types.contains("port: int & >= 1 & <= 65535"),
            "{}",
            g.types
        );
    }

    #[test]
    fn number_minimum_maximum_become_a_decimal_range() {
        let spec = r##"{ "definitions": { "S": { "type": "object", "required": ["r"],
          "properties": { "r": { "type": "number", "minimum": 0, "maximum": 1 } } } } }"##;
        let g = generate(spec, Some("S")).unwrap();
        assert!(g.types.contains("r: decimal & >= 0 & <= 1"), "{}", g.types);
    }

    #[test]
    fn integer_minimum_only_emits_open_ended_range() {
        let spec = r##"{ "definitions": { "S": { "type": "object", "required": ["n"],
          "properties": { "n": { "type": "integer", "minimum": 1 } } } } }"##;
        let g = generate(spec, Some("S")).unwrap();
        assert!(g.types.contains("n: int & >= 1"), "{}", g.types);
        assert!(!g.types.contains("<="), "{}", g.types);
    }

    #[test]
    fn string_pattern_becomes_a_regex_refinement() {
        let spec = r##"{ "definitions": { "S": { "type": "object", "required": ["name"],
          "properties": { "name": { "type": "string", "pattern": "[a-z]+" } } } } }"##;
        let g = generate(spec, Some("S")).unwrap();
        assert!(
            g.types.contains(r#"name: str & =~ "[a-z]+""#),
            "{}",
            g.types
        );
    }

    #[test]
    fn string_length_bounds_warn_and_are_ignored() {
        // no type-level string-length refinement exists in Mangrove → warn, keep `str`.
        let spec = r##"{ "definitions": { "S": { "type": "object", "required": ["s"],
          "properties": { "s": { "type": "string", "maxLength": 63 } } } } }"##;
        let g = generate(spec, Some("S")).unwrap();
        assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
        assert!(g.types.contains("s: str"), "{}", g.types);
        assert!(!g.types.contains("&"), "{}", g.types);
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

    // ---- regression tests for the review findings ----

    #[test]
    fn dollar_in_keys_and_enums_is_escaped() {
        // `$ref` key and `${HOME}` enum must emit LITERAL strings (Mangrove
        // interpolates `$…` inside "…"), not parse-breaking interpolations.
        let spec = r##"{ "definitions": { "E": { "type": "object", "required": [],
          "properties": {
            "$ref": { "type": "string" },
            "mode": { "type": "string", "enum": ["a$b", "${HOME}"] } } } } }"##;
        let g = generate(spec, Some("E")).unwrap();
        assert!(g.types.contains("\"\\$ref\"?: str"), "{}", g.types);
        assert!(g.types.contains("\"a\\$b\""), "{}", g.types);
        assert!(g.types.contains("\"\\${HOME}\""), "{}", g.types);
    }

    #[test]
    fn dangling_ref_is_opaque_not_a_panic() {
        let spec = r##"{ "definitions": { "A": { "type": "object", "required": ["b"],
          "properties": { "b": { "$ref": "#/definitions/Missing" } } } } }"##;
        let g = generate(spec, Some("A")).unwrap();
        assert!(g.types.contains("b: Json"), "{}", g.types);
        assert!(!g.types.contains("type Missing"));
    }

    #[test]
    fn sanitize_collisions_get_unique_names() {
        let spec = r##"{ "definitions": {
          "a.b": { "type": "object", "required": ["x"], "properties": { "x": { "type": "integer" } } },
          "a_b": { "type": "object", "required": ["y"], "properties": { "y": { "type": "string" } } }
        } }"##;
        let g = generate(spec, None).unwrap();
        // both emit, under distinct names (no duplicate `type a_b`)
        assert_eq!(g.types.matches("type a_b ").count(), 1, "{}", g.types);
        assert!(g.types.contains("type a_b_2 ="), "{}", g.types);
    }

    #[test]
    fn allof_wrapped_ref_resolves_to_the_ref() {
        let spec = r##"{ "definitions": {
          "Meta": { "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } },
          "Dep": { "type": "object", "required": ["metadata"], "properties": {
            "metadata": { "description": "wrapped", "allOf": [ { "$ref": "#/definitions/Meta" } ] } } }
        } }"##;
        let g = generate(spec, Some("Dep")).unwrap();
        assert!(g.types.contains("metadata: Meta"), "{}", g.types); // not OpaqueObject
    }

    #[test]
    fn v3_nullable_array_type_uses_the_non_null_member() {
        let spec = r##"{ "definitions": { "N": { "type": "object", "required": ["s"],
          "properties": { "s": { "type": ["string", "null"] } } } } }"##;
        let g = generate(spec, Some("N")).unwrap();
        assert!(g.types.contains("s: str"), "{}", g.types); // not OpaqueObject
    }

    #[test]
    fn oneof_becomes_a_union() {
        let spec = r##"{ "definitions": { "U": { "type": "object", "required": ["v"],
          "properties": { "v": { "oneOf": [ { "type": "integer" }, { "type": "string" } ] } } } } }"##;
        let g = generate(spec, Some("U")).unwrap();
        assert!(g.types.contains("v: int | str"), "{}", g.types);
    }
}
