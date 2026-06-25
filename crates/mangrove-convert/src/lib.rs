//! Format converters (M5): YAML/TOML ⇄ Mangrove, over `mangrove_core::Value`.
//!
//! - import: `format → Value`, then [`to_mangrove`] renders the value as a
//!   schemaless Mangrove document (D42 — plain data; the user adds a schema).
//! - export: `Value → format`, run on the evaluated value (D46).
//!
//! Numbers never route through `f64` (D45): YAML reals keep their source text, so
//! `0.1` stays exact; integers are arbitrary-precision. Null is rejected (§2.4).

pub mod toml;
pub mod yaml;

use mangrove_core::Value;

/// Render a (schemaless) `Value` as Mangrove document text. The root must be a map
/// (D2). Strings are escaped so a literal `$` cannot become interpolation on
/// re-parse, and keys that are not simple identifiers are quoted.
pub fn to_mangrove(v: &Value) -> Result<String, String> {
    let Value::Map(m) = v else {
        return Err("a Mangrove document root must be a map".into());
    };
    let mut out = String::new();
    for (k, val) in m {
        out.push_str(&render_key(k));
        out.push_str(": ");
        out.push_str(&render_val(val)?);
        out.push('\n');
    }
    Ok(out)
}

fn render_key(k: &str) -> String {
    let simple = !k.is_empty()
        && k.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if simple { k.to_string() } else { render_str(k) }
}

fn render_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // escape `$` so `$name`/`${…}` is never read as interpolation on re-parse
            '$' => out.push_str("\\$"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render_val(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Int(n) => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => render_str(s),
        Value::List(xs) => {
            let parts: Result<Vec<_>, _> = xs.iter().map(render_val).collect();
            format!("[ {} ]", parts?.join(", "))
        }
        Value::Map(m) => {
            let parts: Result<Vec<_>, _> = m
                .iter()
                .map(|(k, val)| render_val(val).map(|s| format!("{}: {s}", render_key(k))))
                .collect();
            format!("{{ {} }}", parts?.join(", "))
        }
        other => return Err(format!("cannot render {other:?} as Mangrove")),
    })
}
