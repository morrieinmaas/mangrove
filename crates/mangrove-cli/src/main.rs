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
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: mangrove [--version | hash <file> | check <file>]");
    ExitCode::from(2)
}

fn cmd_hash(path: &str) -> ExitCode {
    // Compose first (resolve local `use` + spread + unset → one merged value),
    // then resolve/hash as before. A spread-free document composes to itself.
    let doc = match mangrove_compose::compose(std::path::Path::new(path)) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{path}: {msg}");
            return ExitCode::from(1);
        }
    };
    let env = match mangrove_typed::TypeEnv::build(&doc.typedefs, &doc.unitdefs) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{path}: schema error: {msg}");
            return ExitCode::from(1);
        }
    };
    // The content address is the schema-RESOLVED canonical form (D12): a bound
    // schema resolves unit literals to base integers; a schemaless document
    // hashes its raw data (M1 behaviour) but may not contain unit literals (D14).
    let to_hash = match &doc.schema {
        Some(name) => {
            let Some(ty) = env.resolve(name) else {
                eprintln!("{path}: unknown schema type: {name}");
                return ExitCode::from(1);
            };
            // Only a valid document has a canonical resolved form, so validate
            // before resolving — this also keeps invalid input (e.g. a unit
            // literal in a non-unit field) from ever reaching the encoder.
            let errors = mangrove_typed::validate(&doc.body, ty, &env);
            if !errors.is_empty() {
                for e in &errors {
                    eprintln!("{path}: {e}");
                }
                return ExitCode::from(1);
            }
            match mangrove_typed::resolve(&doc.body, ty, &env) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return ExitCode::from(1);
                }
            }
        }
        None => {
            if contains_unit(&doc.body) {
                eprintln!("{path}: a unit literal requires a schema");
                return ExitCode::from(1);
            }
            doc.body
        }
    };
    println!("{}", mangrove_canonical::hash(&to_hash));
    ExitCode::SUCCESS
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
            eprintln!("{path}: {msg}");
            return ExitCode::from(1);
        }
    };
    let env = match mangrove_typed::TypeEnv::build(&doc.typedefs, &doc.unitdefs) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{path}: schema error: {msg}");
            return ExitCode::from(1);
        }
    };
    let Some(schema_name) = doc.schema else {
        println!("ok (no schema)");
        return ExitCode::SUCCESS;
    };
    let Some(schema_ty) = env.resolve(&schema_name) else {
        eprintln!("{path}: unknown schema type: {schema_name}");
        return ExitCode::from(1);
    };
    // Advisory @deprecated warnings (never affect the exit code).
    for warning in mangrove_typed::deprecations(&doc.body, schema_ty, &env) {
        eprintln!("warning: {warning}");
    }
    let errors = mangrove_typed::validate(&doc.body, schema_ty, &env);
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
