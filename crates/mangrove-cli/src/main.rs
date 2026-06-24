//! The `mangrove` command-line tool.
//!   `mangrove --version`       — print the version
//!   `mangrove hash <file>`     — print the BLAKE3 content address of an L0 document

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
            None => {
                eprintln!("usage: mangrove hash <file>");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: mangrove [--version | hash <file>]");
            ExitCode::from(2)
        }
    }
}

fn cmd_hash(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
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
