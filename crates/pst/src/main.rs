//! pst — local-first personal prompt library.
//!
//! Thin binary: parse argv (incl. `--VAR=value` prescan in P2.2), dispatch to
//! commands. All real work lives in the library (`pst::`).

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pst", version, about = "Local-first personal prompt library", long_about = None)]
struct Cli {
    /// Output machine-readable JSON
    #[arg(long, short, global = true)]
    json: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,
}

fn main() {
    let _cli = Cli::parse();
    // Command surface lands with bead P2.x; scaffold only proves the build.
    println!("pst scaffold — command surface pending");
}
