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

fn read(path: &str) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|e| {
        eprintln!("{path}: {e}");
        ExitCode::from(1)
    })
}

fn cmd_hash(path: &str) -> ExitCode {
    let src = match read(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    match mangrove_syntax::parse(&src) {
        Ok(value) => {
            println!("{}", mangrove_canonical::hash(&value));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}:{e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_check(path: &str) -> ExitCode {
    let src = match read(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let doc = match mangrove_syntax::parse_document(&src) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}:{e}");
            return ExitCode::from(1);
        }
    };
    let env = match mangrove_typed::TypeEnv::build(&doc.typedefs) {
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
