//! pst — local-first personal prompt library.
//!
//! Thin binary: parse argv (incl. `--VAR=value` prescan), dispatch to
//! commands. All real work lives in the library (`pst::`).

use anyhow::Result;
use clap::{CommandFactory, Parser};
use pst::argv::{Prescan, prescan};
use pst::storage::database::Database;
use pst::storage::resolve::{ResolveOutcome, resolve};

#[derive(Parser, Debug)]
#[command(
    name = "pst",
    version,
    about = "Local-first personal prompt library",
    long_about = None,
    disable_help_flag = true,
    no_binary_name = true
)]
struct Cli {
    /// Output machine-readable JSON
    #[arg(long, short = 'j')]
    json: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Print help
    #[arg(long = "help", short = 'h')]
    help: bool,

    #[command(subcommand)]
    cmd: Option<Sub>,
}

/// Catch-all: any bare positional word is a direct-mode query.
#[derive(clap::Subcommand, Debug)]
enum Sub {
    #[command(external_subcommand)]
    Direct(Vec<String>),
}

fn emit_error(payload: serde_json::Value) -> i32 {
    println!("{}", serde_json::to_string(&payload).unwrap_or_default());
    1
}

/// Direct mode: resolve query and print content per output contract §6.
fn run_direct(db: &Database, query: &str, as_json: bool) -> Result<i32> {
    match resolve(db, query)? {
        ResolveOutcome::Hit {
            id: _,
            title: _,
            content,
            ..
        } => {
            if as_json {
                let prompt = db
                    .get_prompt(&content.id)?
                    .unwrap_or_else(|| unreachable!());
                let payload = serde_json::json!({
                    "id": prompt.id,
                    "title": prompt.title,
                    "content": prompt.content,
                    "description": prompt.description,
                    "category": prompt.category,
                    "tags": prompt.tags,
                    "variables": prompt.variables,
                    "version": prompt.version,
                    "use_count": prompt.use_count,
                    "last_used_at": prompt.last_used_at,
                });
                println!("{payload}");
            } else {
                println!("{}", content.raw);
            }
            Ok(0)
        }
        ResolveOutcome::Ambiguous {
            query: q,
            candidates,
        } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "ambiguous",
                    "query": q,
                    "candidates": ids,
                })
            );
            Ok(1)
        }
        ResolveOutcome::NotFound { query: q } => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "not_found", "query": q })
            );
            Ok(1)
        }
    }
}

fn main() -> std::process::ExitCode {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    // Pre-scan for --VAR=value tokens BEFORE clap sees argv.
    let prescan: Prescan = prescan(&raw_args);
    if !prescan.reserved_errors.is_empty() {
        for tok in &prescan.reserved_errors {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "reserved_flag_value",
                    "token": tok,
                    "hint": "reserved flags do not take inline values"
                })
            );
        }
        return std::process::ExitCode::from(2);
    }

    let cli = match Cli::try_parse_from(&prescan.argv) {
        Ok(c) => c,
        Err(e) => {
            // clap's own error rendering (includes help/version requests).
            let _ = e.print();
            return if e.exit_code() == 0 {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::from(2)
            };
        }
    };

    if cli.help || prescan.argv.is_empty() {
        let _ = Cli::command().print_help();
        return std::process::ExitCode::SUCCESS;
    }

    // Open the database at PST_HOME.
    let home = std::env::var("PST_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            directories::ProjectDirs::from("com", "promptstorage", "pst")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
        });
    let db = match Database::open(&home) {
        Ok(db) => db,
        Err(e) => {
            let code = emit_error(serde_json::json!({
                "error": "database_error",
                "message": e.to_string(),
            }));
            return std::process::ExitCode::from(code as u8);
        }
    };

    let query = match &cli.cmd {
        Some(Sub::Direct(words)) => words.first().map(String::as_str),
        _ => None,
    };
    let Some(query) = query else {
        let _ = Cli::command().print_help();
        return std::process::ExitCode::SUCCESS;
    };

    match run_direct(&db, query, cli.json) {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "internal", "message": e.to_string() })
            );
            std::process::ExitCode::from(1)
        }
    }
}
