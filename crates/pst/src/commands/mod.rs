//! Command modules — presentation logic only (plan §3 architecture rule).
pub mod core;
pub mod discovery;
pub mod export;

use crate::storage::database::Database;
use anyhow::Result;

/// `pst copy <query> [--fill] [--VAR=value...]` — resolve, optionally render,
/// copy to clipboard. stdout stays data-only per contract §6.
pub fn copy_cmd(
    db: &Database,
    query: &str,
    fill: bool,
    as_json: bool,
    prescan_vars: &[(String, String)],
) -> Result<i32> {
    use crate::storage::resolve::{ResolveOutcome, resolve};

    match resolve(db, query)? {
        ResolveOutcome::Hit { id, title, .. } => {
            let prompt = db.get_prompt(&id)?.expect("resolved exists");

            // Render when variables were provided or --fill given.
            let final_text = if !prescan_vars.is_empty() || (fill && !prompt.variables.is_empty()) {
                let cwd = std::env::current_dir().unwrap_or_default();
                match crate::render::build_values(&prompt, prescan_vars, &cwd) {
                    Ok(vals) => crate::render::render_content(&prompt.content, &vals).0,
                    Err(e) => {
                        eprintln!(
                            "{}",
                            serde_json::json!({ "error": "variable_error", "message": e.to_string() })
                        );
                        return Ok(1);
                    }
                }
            } else if prompt.variables.is_empty() {
                prompt.content.clone()
            } else {
                // Has declared variables but none provided → render with
                // defaults only; missing required still surfaces via render
                // passthrough (v1 keeps it lenient).
                prompt.content.clone()
            };
            let _ = fill;

            match crate::clipboard::copy_to_clipboard(&final_text) {
                Ok(()) => {
                    eprintln!("Copied: {title}");
                    if as_json {
                        println!(
                            "{}",
                            serde_json::json!({ "copied": true, "id": id, "title": title })
                        );
                    }
                    Ok(0)
                }
                Err(tool_err) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "error": "clipboard_failed",
                            "message": tool_err,
                            "hint": "install pbcopy/xclip/xsel/wl-copy"
                        })
                    );
                    Ok(1)
                }
            }
        }
        other => {
            // Reuse the same stderr shapes as direct mode.
            match other {
                ResolveOutcome::Ambiguous {
                    query: q,
                    candidates,
                } => {
                    let ids: Vec<serde_json::Value> = candidates
                        .iter()
                        .map(|c| serde_json::json!({"id": c.id, "title": c.title}))
                        .collect();
                    eprintln!(
                        "{}",
                        serde_json::json!({ "error": "ambiguous", "query": q, "candidates": ids })
                    );
                }
                ResolveOutcome::NotFound { query } => {
                    eprintln!(
                        "{}",
                        serde_json::json!({ "error": "not_found", "query": query })
                    );
                }
                _ => unreachable!(),
            }
            Ok(1)
        }
    }
}

/// `pst export` — JSONL to file, or markdown files into a directory.
#[allow(clippy::too_many_arguments)]
pub fn export_cmd(
    db: &Database,
    ids: &[String],
    all: bool,
    format: &str,
    out: Option<&str>,
    stdout: bool,
    as_json: bool,
) -> Result<i32> {
    use crate::storage::resolve::{ResolveOutcome, resolve};

    if format == "jsonl" {
        let Some(path) = out else {
            eprintln!(
                "{}",
                serde_json::json!({"error":"missing_out","hint":"--out PATH required for jsonl"})
            );
            return Ok(1);
        };
        let n = crate::commands::export::export_jsonl(db, std::path::Path::new(path))?;
        if as_json || !atty::is(atty::Stream::Stdout) {
            println!(
                "{}",
                serde_json::json!({ "exported": n, "format": "jsonl", "path": path })
            );
        } else {
            println!("Exported {n} prompts → {path}");
        }
        return Ok(0);
    }

    // markdown export
    if stdout {
        let targets: Vec<String> = if all {
            db.list_prompts_filtered(None, None, false)?
                .into_iter()
                .map(|p| p.id)
                .collect()
        } else {
            ids.to_vec()
        };
        for id in &targets {
            match resolve(db, id)? {
                ResolveOutcome::Hit { id: rid, .. } => {
                    let p = db.get_prompt(&rid)?.expect("exists");
                    print!("{}", crate::commands::export::format_markdown(&p));
                }
                _ => {
                    eprintln!("{}", serde_json::json!({"error":"not_found","query":id}));
                    return Ok(1);
                }
            }
        }
        return Ok(0);
    }

    let Some(dir) = out else {
        eprintln!(
            "{}",
            serde_json::json!({"error":"missing_out","hint":"--out DIR required for md export"})
        );
        return Ok(1);
    };
    let dir_path = std::path::Path::new(dir);
    let files = crate::commands::export::export_markdown_all(db, dir_path)?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        println!(
            "{}",
            serde_json::json!({
                "exported": files.len(),
                "files": files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
            })
        );
    } else {
        println!("Exported {} markdown files → {}", files.len(), dir);
    }
    Ok(0)
}

/// `pst import <path> [--merge|--replace]`
pub fn import_cmd(
    db: &Database,
    path: &std::path::Path,
    mode: crate::commands::export::ImportMode,
) -> Result<i32> {
    match crate::commands::export::import_jsonl(db, path, mode) {
        Ok(n) => {
            println!(
                "{}",
                serde_json::json!({ "imported": n, "mode": if mode == crate::commands::export::ImportMode::Replace { "replace" } else { "merge" } })
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "import_failed", "message": e.to_string() })
            );
            Ok(1)
        }
    }
}
