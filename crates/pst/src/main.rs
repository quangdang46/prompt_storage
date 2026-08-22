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

#[derive(clap::Subcommand, Debug)]
enum ColArgs {
    #[command(external_subcommand)]
    Args(Vec<String>),
}

/// Real subcommands plus a catch-all for bare direct-mode queries.
#[derive(clap::Subcommand, Debug)]
enum Sub {
    /// Show prompt metadata + preview (human view)
    Show {
        query: String,
    },

    /// Create a new prompt
    New {
        id: String,
        #[arg(long, short = 't')]
        title: Option<String>,
        #[arg(long, short = 'd')]
        desc: Option<String>,
        #[arg(long, short = 'c')]
        category: Option<String>,
        #[arg(long, short = 'g')]
        tag: Vec<String>,
        /// Content source: FILE path or `-` for stdin
        #[arg(long, short = 'f')]
        from: Option<String>,
        #[arg(long)]
        force: bool,
    },

    /// Delete a prompt
    Rm {
        id: String,
        #[arg(long)]
        force: bool,
    },

    /// Add alias(es) to a prompt
    Alias {
        id: String,
        aliases: Vec<String>,
    },

    /// Remove alias(es)
    Unalias {
        aliases: Vec<String>,
    },

    /// List prompts
    List {
        #[arg(long, short = 'c')]
        category: Option<String>,
        #[arg(long, short = 't')]
        tag: Option<String>,
        #[arg(long)]
        featured: bool,
        #[arg(long, short = 'l')]
        limit: Option<usize>,
    },

    /// Export prompts (JSONL backup or markdown files)
    Export {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, short = 'f', default_value = "jsonl")]
        format: String,
        #[arg(long, short = 'o')]
        out: Option<String>,
        #[arg(long)]
        stdout: bool,
    },

    /// Import prompts from JSONL backup
    Import {
        path: String,
        #[arg(long, default_value_t = false, conflicts_with = "replace")]
        merge: bool,
        #[arg(long, default_value_t = false)]
        replace: bool,
    },

    /// Interactive picker (TUI)
    #[command(alias = "i")]
    Interactive,

    /// Collection management
    Collection {
        #[command(subcommand)]
        args: ColArgs,
    },

    /// Install the prompt-storage integration skill for AI agents
    Install {
        #[arg(long)]
        project: bool,
        #[arg(long)]
        personal: bool,
        #[arg(long)]
        force: bool,
    },

    /// Remove ONLY the integration skill (never touches your library)
    Uninstall {
        #[arg(long)]
        project: bool,
        #[arg(long)]
        personal: bool,
    },

    /// Manage configuration
    Config {
        action: String,
        key: Option<String>,
        value: Option<String>,
    },

    /// Show library status
    Status,

    /// Health checks + repair
    Doctor {
        #[arg(long)]
        fix: bool,
    },

    Categories,

    /// Tag counts
    Tags,

    /// Copy prompt content to clipboard (with optional variable filling)
    Copy {
        query: String,
        /// Interactively fill missing variables (TTY only)
        #[arg(long)]
        fill: bool,
    },

    /// Direct mode catch-all: any bare word(s) resolve as a query
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
                let prompt = db.get_prompt(&content.id)?.expect("resolved exists");
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
                // Contract: content + exactly one trailing \n. If the stored
                // content already ends with a newline, print as-is; otherwise
                // append one. Never two.
                if content.raw.ends_with('\n') {
                    print!("{}", content.raw);
                } else {
                    println!("{}", content.raw);
                }
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

fn home_dir() -> std::path::PathBuf {
    std::env::var("PST_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            directories::ProjectDirs::from("com", "promptstorage", "pst")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        })
}

fn open_db() -> Result<Database> {
    Database::open(&home_dir())
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
    let home = home_dir();
    let db = match open_db() {
        Ok(db) => db,
        Err(e) => {
            let code = emit_error(serde_json::json!({
                "error": "database_error",
                "message": e.to_string(),
            }));
            return std::process::ExitCode::from(code as u8);
        }
    };

    let result: Result<i32> = match cli.cmd {
        Some(Sub::Show { query }) => pst::commands::core::cmd_show(&db, &query, cli.json),
        Some(Sub::New {
            id,
            title,
            desc,
            category,
            tag,
            from,
            force,
        }) => pst::commands::core::cmd_new(
            &db,
            pst::commands::core::NewArgs {
                id: &id,
                title,
                description: desc,
                category,
                tags: tag,
                from,
                force,
            },
        ),
        Some(Sub::Rm { id, force }) => pst::commands::core::cmd_rm(&db, &id, force),
        Some(Sub::Alias { id, aliases }) => pst::commands::core::cmd_alias(&db, &id, &aliases),
        Some(Sub::Unalias { aliases }) => pst::commands::core::cmd_unalias(&db, &aliases),
        Some(Sub::List {
            category,
            tag,
            featured,
            limit,
        }) => pst::commands::discovery::cmd_list(
            &db,
            category.as_deref(),
            tag.as_deref(),
            featured,
            limit,
            cli.json,
        ),
        Some(Sub::Copy { query, fill }) => {
            pst::commands::copy_cmd(&db, &query, fill, cli.json, &prescan.vars)
        }
        Some(Sub::Export {
            ids,
            all,
            format,
            out,
            stdout,
        }) => pst::commands::export_cmd(&db, &ids, all, &format, out.as_deref(), stdout, cli.json),
        Some(Sub::Import { path, replace, .. }) => {
            let mode = if replace {
                pst::commands::export::ImportMode::Replace
            } else {
                pst::commands::export::ImportMode::Merge
            };
            pst::commands::import_cmd(&db, std::path::Path::new(&path), mode)
        }
        Some(Sub::Collection {
            args: ColArgs::Args(a),
        }) => pst::commands::collections::cmd_collection(&db, &a, cli.json),
        Some(Sub::Install {
            project,
            personal,
            force,
        }) => {
            let root = pst::skills::agents::resolve_root(project, personal);
            match pst::skills::agents::install(&root, force) {
                Ok(report) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "installed": true,
                            "canonical": report.canonical.display().to_string(),
                            "adapters": report.installed,
                        })
                    );
                    Ok(0)
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"error":"install_failed","message":e.to_string()})
                    );
                    Ok(1)
                }
            }
        }
        Some(Sub::Uninstall { project, personal }) => {
            let root = pst::skills::agents::resolve_root(project, personal);
            match pst::skills::agents::uninstall(&root) {
                Ok(n) => {
                    println!(
                        "{}",
                        serde_json::json!({ "uninstalled": true, "artifacts": n })
                    );
                    Ok(0)
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"error":"uninstall_failed","message":e.to_string()})
                    );
                    Ok(1)
                }
            }
        }
        Some(Sub::Config { action, key, value }) => pst::commands::system::cmd_config(
            &home,
            &action,
            key.as_deref(),
            value.as_deref(),
            cli.json,
        ),
        Some(Sub::Status) => pst::commands::system::cmd_status(&db, &home, cli.json),
        Some(Sub::Doctor { fix }) => pst::commands::system::cmd_doctor(&db, &home, fix, cli.json),
        Some(Sub::Interactive) => {
            if cli.json {
                eprintln!(
                    "{}",
                    serde_json::json!({"error":"tty_required","hint":"interactive picker needs a terminal"})
                );
                Ok(1)
            } else {
                match pst::tui::run_tui(&db) {
                    Ok(pst::tui::TuiAction::Copy(content)) => {
                        // already copied inside run_tui; nothing on stdout
                        let _ = content;
                        Ok(0)
                    }
                    Ok(pst::tui::TuiAction::Print(content)) => {
                        println!("{content}");
                        Ok(0)
                    }
                    _ => Ok(0),
                }
            }
        }
        Some(Sub::Categories) => pst::commands::discovery::cmd_categories(&db, cli.json),
        Some(Sub::Tags) => pst::commands::discovery::cmd_tags(&db, cli.json),
        // Bare positional words → direct mode.
        None | Some(Sub::Direct(_)) => {
            let query = match &cli.cmd {
                Some(Sub::Direct(words)) => words.first().cloned(),
                _ => None,
            };
            match query {
                Some(q) => run_direct(&db, &q, cli.json),
                None => {
                    let _ = Cli::command().print_help();
                    Ok(0)
                }
            }
        }
    };

    match result {
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
