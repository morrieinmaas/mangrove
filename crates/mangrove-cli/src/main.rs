//! The `mangrove` command-line tool. At M0 it only reports its version;
//! subcommands (`hash`, `validate`, `build`) arrive with later milestones.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => {
            println!("mangrove {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!("usage: mangrove --version");
            std::process::exit(2);
        }
    }
}
