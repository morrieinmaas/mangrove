//! The `mangrove` command-line tool.
//!   `mangrove --version`                        — print the version
//!   `mangrove hash <file>`                      — print the BLAKE3 content address of an L0 document
//!   `mangrove check <file>`                     — validate a document against its bound schema
//!   `mangrove update <file>`                    — resolve + pin namespaced imports into mangrove.lock
//!   `mangrove import <file>`                    — convert a YAML/TOML file to a Mangrove document
//!                                                 (multi-doc YAML streams import as a Mangrove list)
//!   `mangrove export <file> --to yaml`          — evaluate a document and emit YAML (default)
//!   `mangrove export <file> --to yaml-stream`   — emit a Value::List as a YAML multi-doc stream
//!   `mangrove export <file> --to toml`          — evaluate a document and emit TOML
//!   `mangrove gen-openapi [--k8s] <spec>…`      — generate Mangrove types from OpenAPI spec(s)
//!   `mangrove fmt <file>…`                      — format documents (--check to gate; - for stdin)
//!   `mangrove lsp`                              — run the language server over stdio (editors)

use mangrove_core::error::ValidationError;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("mangrove {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("hash") => match args.get(2) {
            Some(path) => cmd_hash(path),
            None => usage(),
        },
        Some("check") => match args.get(2) {
            Some(path) => cmd_check(path),
            None => usage(),
        },
        Some("update") => match args.get(2) {
            Some(path) => cmd_update(path),
            None => usage(),
        },
        Some("import") => match args.get(2) {
            Some(path) => cmd_import(path),
            None => usage(),
        },
        Some("export") => match args.get(2) {
            // `export <file> [--to <fmt>]`
            Some(path) => {
                let to = if args.get(3).map(String::as_str) == Some("--to") {
                    args.get(4).map(String::as_str)
                } else {
                    None
                };
                cmd_export(path, to)
            }
            None => usage(),
        },
        Some("gen-openapi") => cmd_gen_openapi_multi(&args[2..]),
        Some("fmt") => cmd_fmt(&args[2..]),
        Some("lsp") => cmd_lsp(),
        _ => usage(),
    }
}

/// `mangrove lsp` — run the language server over stdio (read-only, no network).
/// Editors spawn this; it speaks LSP on stdin/stdout until shutdown.
fn cmd_lsp() -> ExitCode {
    match mangrove_lsp::server::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lsp: {e}");
            ExitCode::from(1)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: mangrove [--version | hash <file> | check <file> | update <file> \
         | import <file.yaml|.toml> | export <file.mang> [--to yaml|yaml-stream|toml] \
         | gen-openapi [--k8s] <spec>... [--root <Def>]... \
         | fmt <file…> | fmt --check <file…> | fmt - | lsp]"
    );
    ExitCode::from(2)
}

/// `mangrove gen-openapi [--k8s] <spec>... [--root <Def>]...`
///
/// Parses args: `--k8s` (global flag), positional spec paths, repeatable `--root` values.
/// Single file: root is optional (back-compat). Multiple files: roots must match files 1:1.
fn cmd_gen_openapi_multi(args: &[String]) -> ExitCode {
    let mut k8s = false;
    let mut files: Vec<&str> = Vec::new();
    let mut roots: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--k8s" => k8s = true,
            "--root" => {
                i += 1;
                match args.get(i) {
                    Some(r) => roots.push(r.as_str()),
                    None => {
                        eprintln!("gen-openapi: --root requires a value");
                        return ExitCode::from(2);
                    }
                }
            }
            arg if arg.starts_with("--") => {
                eprintln!("gen-openapi: unknown flag `{arg}`");
                return ExitCode::from(2);
            }
            path => files.push(path),
        }
        i += 1;
    }

    if files.is_empty() {
        return usage();
    }

    if files.len() > 1 && roots.len() != files.len() {
        eprintln!(
            "gen-openapi: {} spec(s) but {} --root(s); with multiple specs, \
             provide exactly one --root per spec (positionally matched)",
            files.len(),
            roots.len()
        );
        return ExitCode::from(2);
    }

    // Read all files.
    let mut texts: Vec<String> = Vec::new();
    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(t) => texts.push(t),
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::from(1);
            }
        }
    }

    // Build GenInput list.
    let inputs: Vec<mangrove_openapi::GenInput> = files
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            let root = if files.len() == 1 {
                roots.first().copied()
            } else {
                Some(roots[idx])
            };
            mangrove_openapi::GenInput {
                spec_json: texts[idx].as_str(),
                root,
                k8s,
            }
        })
        .collect();

    match mangrove_openapi::generate_many(&inputs) {
        Ok(g) => {
            for w in &g.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", g.types);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// `mangrove update <file>` — resolve every reachable namespaced `use`, hash each
/// source, and write `mangrove.lock` next to the document (§5.2; the only writer).
fn cmd_update(path: &str) -> ExitCode {
    let refs = match mangrove_compose::lock_references(std::path::Path::new(path)) {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("{path}: {msg}");
            return ExitCode::from(1);
        }
    };
    let count = refs.len();
    let mut lock = mangrove_resolve::Lockfile::default();
    for (k, v) in refs {
        lock.insert(k, v);
    }
    let lock_path = std::path::Path::new(path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mangrove.lock");
    if let Err(e) = std::fs::write(&lock_path, lock.to_toml()) {
        eprintln!("{}: {e}", lock_path.display());
        return ExitCode::from(1);
    }
    println!("wrote {count} pin(s) to {}", lock_path.display());
    ExitCode::SUCCESS
}

/// Compose → eval → (validate + resolve against the schema) → the canonical value
/// (D12). The shared pipeline behind `hash` and `export`. Errors are pre-formatted
/// (already prefixed with the path where useful) for the caller to print.
fn evaluate(path: &str) -> Result<mangrove_core::Value, String> {
    // compose errors already carry the offending file's path — don't double-prefix.
    let doc = mangrove_compose::compose(std::path::Path::new(path))?;
    let imports = type_imports(&doc);
    let env = mangrove_typed::TypeEnv::build_with_imports(&doc.typedefs, &doc.unitdefs, &imports)
        .map_err(|m| format!("{path}: schema error: {m}"))?;
    // L3 eval: reduce params/references/calls to plain values (D35).
    let modules = build_modules(&doc.modules).map_err(|e| format!("{path}: {e}"))?;
    let body = mangrove_typed::eval(&doc.body, &doc.params, &doc.fns, &env, &modules)
        .map_err(|e| format!("{path}: {e}"))?;
    // Reject a surviving `Value::Unset` before it reaches the CBOR encoder's
    // panic guard. `unset` only removes a binding during composition — it is
    // never a final value, whether at the document root, in a list element, or
    // nested inside a map.  This runs for both schema and no-schema paths.
    if contains_unset(&body) {
        return Err(format!(
            "{path}: `unset` is not a value (it only removes a binding during composition)"
        ));
    }
    match &doc.schema {
        Some(name) => {
            let ty = effective_schema(name, &doc.schema_narrow, &env)
                .map_err(|e| format!("{path}: {e}"))?;
            // Validate before resolving — the canonical resolved form exists only
            // for a valid document, and this keeps invalid input out of the encoder.
            let errors = mangrove_typed::validate(&body, &ty, &env);
            if !errors.is_empty() {
                return Err(errors
                    .iter()
                    .map(|e| format!("{path}: {e}"))
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
            mangrove_typed::resolve(&body, &ty, &env).map_err(|e| format!("{path}: {e}"))
        }
        None => {
            if contains_unit(&body) {
                return Err(format!("{path}: a unit literal requires a schema"));
            }
            Ok(body)
        }
    }
}

fn cmd_hash(path: &str) -> ExitCode {
    match evaluate(path) {
        Ok(v) => {
            println!("{}", mangrove_canonical::hash(&v));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// `mangrove import <file.yaml|.toml>` — parse a YAML/TOML file into plain data and
/// print it as a schemaless Mangrove document (D42). Format is chosen by extension.
fn cmd_import(path: &str) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    let value = match format_of(path) {
        Some(Format::Yaml) => mangrove_convert::yaml::import(&text),
        Some(Format::Toml) => mangrove_convert::toml::import(&text),
        None => Err(format!(
            "{path}: unknown format (expected .yaml/.yml/.toml)"
        )),
    };
    match value.and_then(|v| mangrove_convert::to_mangrove(&v)) {
        Ok(doc) => {
            print!("{doc}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::from(1)
        }
    }
}

/// `mangrove export <file.mang> --to yaml|yaml-stream|toml` — evaluate a document
/// and print its canonical value in the target format (D46). Defaults to YAML.
///
/// `--to yaml-stream`: if the document body is a `Value::List`, each element is
/// emitted as a separate YAML document separated by `---` (a multi-doc stream).
/// Non-list values are emitted as a single-document stream, identical to `--to yaml`.
fn cmd_export(path: &str, to: Option<&str>) -> ExitCode {
    let value = match evaluate(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let out = match to {
        None | Some("yaml") => mangrove_convert::yaml::export(&value),
        Some("yaml-stream") => mangrove_convert::yaml::export_stream(&value),
        Some("toml") => mangrove_convert::toml::export(&value),
        Some(other) => Err(format!(
            "unknown target format `{other}` (expected yaml, yaml-stream, or toml)"
        )),
    };
    match out {
        Ok(s) => {
            print!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::from(1)
        }
    }
}

enum Format {
    Yaml,
    Toml,
}

fn format_of(path: &str) -> Option<Format> {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("yaml") | Some("yml") => Some(Format::Yaml),
        Some("toml") => Some(Format::Toml),
        _ => None,
    }
}

/// The effective schema type: the named type, or — for `schema Base & {…}` — the
/// narrowed type (checked `New <: Old`, §5.5).
fn effective_schema(
    name: &str,
    narrow: &Option<mangrove_syntax::Type>,
    env: &mangrove_typed::TypeEnv,
) -> Result<mangrove_syntax::Type, String> {
    match narrow {
        Some(n) => mangrove_compose::narrowed_schema(
            &mangrove_syntax::Type::Named(name.to_string()),
            n,
            env,
        ),
        None => env
            .resolve(name)
            .cloned()
            .ok_or_else(|| format!("unknown schema type: {name}")),
    }
}

/// The type/unit definitions a document imports: each `use`d module under
/// `alias.Name` (M6a), and each version-pinned package under `alias@version.Name`
/// (§5.6, M6b). The composed doc already keyed `pinned` as `alias@version`.
fn type_imports(
    doc: &mangrove_compose::Composed,
) -> Vec<(
    &str,
    &[mangrove_syntax::TypeDef],
    &[mangrove_syntax::UnitDef],
)> {
    doc.modules
        .iter()
        .chain(doc.pinned.iter())
        .map(|(q, c)| (q.as_str(), c.typedefs.as_slice(), c.unitdefs.as_slice()))
        .collect()
}

/// Build the eval module map (alias → callable module) from a composed document's
/// `use` aliases (§6.1, M4d.2). Recurses so a module that calls a helper module
/// carries that helper (B1), and computes each module's effective schema so its
/// instantiated body gets validated + unit-resolved (B2).
fn build_modules(
    modules: &std::collections::BTreeMap<String, mangrove_compose::Composed>,
) -> Result<std::collections::BTreeMap<String, mangrove_typed::Module>, String> {
    let mut out = std::collections::BTreeMap::new();
    for (alias, c) in modules {
        let types = mangrove_typed::TypeEnv::build(&c.typedefs, &c.unitdefs)?;
        let schema = match &c.schema {
            Some(name) => Some(effective_schema(name, &c.schema_narrow, &types)?),
            None => None,
        };
        out.insert(
            alias.clone(),
            mangrove_typed::Module {
                params: c.params.clone(),
                fns: c.fns.clone(),
                body: c.body.clone(),
                types,
                schema,
                modules: build_modules(&c.modules)?,
            },
        );
    }
    Ok(out)
}

/// Whether a value tree contains an unresolved unit literal (schemaless guard, D14).
fn contains_unit(v: &mangrove_core::Value) -> bool {
    use mangrove_core::Value;
    match v {
        Value::Unit { .. } => true,
        Value::List(xs) => xs.iter().any(contains_unit),
        Value::Map(m) => m.values().any(contains_unit),
        _ => false,
    }
}

/// Whether a value tree contains a surviving `Value::Unset`. `unset` is only
/// ever meaningful as a binding-time directive that removes a key during
/// composition; it must never appear in a final evaluated body. This guard
/// covers the root, list elements, and nested map values.
fn contains_unset(v: &mangrove_core::Value) -> bool {
    use mangrove_core::Value;
    match v {
        Value::Unset => true,
        Value::List(xs) => xs.iter().any(contains_unset),
        Value::Map(m) => m.values().any(contains_unset),
        _ => false,
    }
}

fn cmd_check(path: &str) -> ExitCode {
    let doc = match mangrove_compose::compose(std::path::Path::new(path)) {
        Ok(c) => c,
        Err(msg) => {
            // compose errors already carry the file path.
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    let imports = type_imports(&doc);
    let env =
        match mangrove_typed::TypeEnv::build_with_imports(&doc.typedefs, &doc.unitdefs, &imports) {
            Ok(e) => e,
            Err(msg) => {
                eprintln!("{path}: schema error: {msg}");
                return ExitCode::from(1);
            }
        };
    // L3 eval: reduce params/references/calls before validating (D35).
    let modules = match build_modules(&doc.modules) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    let body = match mangrove_typed::eval(&doc.body, &doc.params, &doc.fns, &env, &modules) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    let Some(schema_name) = doc.schema else {
        println!("ok (no schema)");
        return ExitCode::SUCCESS;
    };
    let schema_ty = match effective_schema(&schema_name, &doc.schema_narrow, &env) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    // Advisory @deprecated warnings (never affect the exit code).
    for warning in mangrove_typed::deprecations(&body, &schema_ty, &env) {
        eprintln!("warning: {warning}");
    }
    let errors = mangrove_typed::validate(&body, &schema_ty, &env);
    if errors.is_empty() {
        println!("ok");
        ExitCode::SUCCESS
    } else {
        for e in &errors {
            print_error(e);
        }
        ExitCode::from(1)
    }
}

/// `mangrove fmt <file…> | --check <file…> | -`
///
/// - `fmt -`             : read stdin, write formatted text to stdout.
/// - `fmt --check <file…>`: print paths of files that would change; exit 1 if any.
/// - `fmt <file…>`       : rewrite each file in place if its content would change.
fn cmd_fmt(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("-") => {
            let mut src = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut src).expect("read stdin");
            print!("{}", mangrove_fmt::format_str(&src).text);
            ExitCode::SUCCESS
        }
        Some("--check") => {
            let mut changed = false;
            for path in &args[1..] {
                let src = match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        return ExitCode::from(2);
                    }
                };
                if mangrove_fmt::format_str(&src).text != src {
                    eprintln!("{path}");
                    changed = true;
                }
            }
            if changed {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Some(_) => {
            for path in args {
                let src = match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        return ExitCode::from(2);
                    }
                };
                let out = mangrove_fmt::format_str(&src).text;
                if out != src {
                    if let Err(e) = std::fs::write(path, out) {
                        eprintln!("{path}: {e}");
                        return ExitCode::from(2);
                    }
                }
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("usage: mangrove fmt <file…> | --check <file…> | -");
            ExitCode::from(2)
        }
    }
}

/// Print one structured error in the spec §12 layout.
fn print_error(e: &ValidationError) {
    let path = if e.path.is_empty() { "(root)" } else { &e.path };
    println!("error: {path}");
    println!("  got:      {}", e.got);
    println!("  expected: {}", e.expected);
    if let Some(f) = &e.failed {
        println!("  failed:   {f}");
    }
    if let Some(m) = &e.message {
        println!("  message:  {m}");
    }
}

#[cfg(test)]
mod tests {
    use super::contains_unset;
    use mangrove_core::Value;
    use std::collections::BTreeMap;

    #[test]
    fn contains_unset_direct() {
        assert!(contains_unset(&Value::Unset));
    }

    #[test]
    fn contains_unset_in_list() {
        let list = Value::List(vec![
            Value::Int(1.into()),
            Value::Unset,
            Value::Int(3.into()),
        ]);
        assert!(contains_unset(&list));
    }

    #[test]
    fn contains_unset_in_map_value() {
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), Value::Unset);
        assert!(contains_unset(&Value::Map(m)));
    }

    #[test]
    fn contains_unset_false_for_plain_values() {
        assert!(!contains_unset(&Value::Int(42.into())));
        assert!(!contains_unset(&Value::Bool(true)));
        assert!(!contains_unset(&Value::Str("hello".into())));
        let list = Value::List(vec![Value::Int(1.into()), Value::Int(2.into())]);
        assert!(!contains_unset(&list));
    }
}
