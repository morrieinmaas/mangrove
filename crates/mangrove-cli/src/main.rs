//! The `mangrove` command-line tool.
//!   `mangrove --version`       — print the version
//!   `mangrove hash <file>`     — print the BLAKE3 content address of an L0 document
//!   `mangrove check <file>`    — validate a document against its bound schema

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
        Some("gen-openapi") => match args.get(2) {
            // `gen-openapi <spec.json> [--root <Definition>]`
            Some(path) => {
                if args.get(3).map(String::as_str) == Some("--root") {
                    match args.get(4) {
                        Some(root) => cmd_gen_openapi(path, Some(root)),
                        None => usage(), // bare `--root` with no value
                    }
                } else {
                    cmd_gen_openapi(path, None)
                }
            }
            None => usage(),
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: mangrove [--version | hash <file> | check <file> | update <file> \
         | import <file.yaml|.toml> | export <file.mang> [--to yaml|toml] \
         | gen-openapi <spec.json> [--root <Definition>]]"
    );
    ExitCode::from(2)
}

/// `mangrove gen-openapi <spec.json> [--root <Def>]` — emit Mangrove `type`s for
/// an OpenAPI spec (e.g. the Kubernetes API). Warnings go to stderr, types to stdout.
fn cmd_gen_openapi(path: &str, root: Option<&str>) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    match mangrove_openapi::generate(&text, root) {
        Ok(g) => {
            for w in &g.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", g.types);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
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

/// `mangrove export <file.mang> --to yaml|toml` — evaluate a document and print its
/// canonical value in the target format (D46). Defaults to YAML.
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
        Some("toml") => mangrove_convert::toml::export(&value),
        Some(other) => Err(format!(
            "unknown target format `{other}` (expected yaml or toml)"
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
