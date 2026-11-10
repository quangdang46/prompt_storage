//! Command modules — presentation logic only (plan §3 architecture rule).
pub mod collections;
pub mod core;
pub mod discovery;
pub mod export;
pub mod system;

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

/// `pst export` — JSONL or markdown, to file/stdout/directory.
/// `--ids`/`--all` select the prompt set for EVERY format (previously only
/// the markdown --stdout branch honored them).
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

    let is_jsonl = match format {
        "jsonl" => true,
        "markdown" | "md" => false,
        other => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "invalid_format",
                    "format": other,
                    "hint": "supported: jsonl, markdown (alias: md)"
                })
            );
            return Ok(1);
        }
    };

    // Resolve the target set once, shared by every format.
    //   --all            → every prompt
    //   --ids a,b        → those prompts (aliases/prefixes resolve)
    //   neither          → every prompt (documented default)
    let targets: Vec<crate::model::Prompt> = if all || ids.is_empty() {
        db.list_prompts_filtered(None, None, false)?
    } else {
        let mut v = Vec::with_capacity(ids.len());
        for id in ids {
            match resolve(db, id)? {
                ResolveOutcome::Hit { id: rid, .. } => match db.get_prompt(&rid)? {
                    Some(p) => v.push(p),
                    None => {
                        eprintln!("{}", serde_json::json!({"error":"not_found","query":id}));
                        return Ok(1);
                    }
                },
                other => return crate::commands::forward_err(other),
            }
        }
        v
    };

    if is_jsonl {
        if stdout {
            let n = crate::storage::jsonl::export_jsonl_to(
                db,
                &targets,
                &mut std::io::stdout().lock(),
            )?;
            if as_json || !atty::is(atty::Stream::Stdout) {
                // stdout already carries the JSONL document; report counts
                // on stderr so stdout stays a clean backup stream.
                eprintln!(
                    "{}",
                    serde_json::json!({ "exported": n, "format": "jsonl", "stdout": true })
                );
            }
            return Ok(0);
        }
        let Some(path) = out else {
            eprintln!(
                "{}",
                serde_json::json!({"error":"missing_out","hint":"--out PATH required for jsonl (or --stdout)"})
            );
            return Ok(1);
        };
        let n =
            crate::storage::jsonl::export_jsonl_prompts(db, std::path::Path::new(path), &targets)?;
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
        for p in &targets {
            print!("{}", crate::commands::export::format_markdown(p));
        }
        return Ok(0);
    }

    let Some(dir) = out else {
        eprintln!(
            "{}",
            serde_json::json!({"error":"missing_out","hint":"--out DIR required for md export (or --stdout)"})
        );
        return Ok(1);
    };
    let result =
        crate::commands::export::export_markdown_filtered(std::path::Path::new(dir), &targets)?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        println!(
            "{}",
            serde_json::json!({
                "exported": result.files.len(),
                "files": result.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
                "skipped": result.skipped,
            })
        );
    } else {
        println!("Exported {} markdown files → {}", result.files.len(), dir);
        for id in &result.skipped {
            eprintln!("skipped unsafe id: {id}");
        }
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

/// `pst render <query> [--VAR=value...]` — print rendered prompt to stdout.
pub fn render_cmd(
    db: &Database,
    query: &str,
    as_json: bool,
    prescan_vars: &[(String, String)],
) -> Result<i32> {
    use crate::storage::resolve::{ResolveOutcome, resolve};

    match resolve(db, query)? {
        ResolveOutcome::Hit { id, .. } => {
            let prompt = db.get_prompt(&id)?.expect("resolved exists");
            let cwd = std::env::current_dir().unwrap_or_default();
            match crate::render::build_values(&prompt, prescan_vars, &cwd) {
                Ok(values) => {
                    let (rendered, _) = crate::render::render_content(&prompt.content, &values);
                    if as_json {
                        println!(
                            "{}",
                            serde_json::json!({ "id": prompt.id, "rendered": rendered })
                        );
                    } else if rendered.ends_with('\n') {
                        print!("{rendered}");
                    } else {
                        println!("{rendered}");
                    }
                    Ok(0)
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"error":"variable_error","message":e.to_string()})
                    );
                    Ok(1)
                }
            }
        }
        other => forward_err(other),
    }
}

/// Shared: emit contract-shaped stderr for non-Hit outcomes.
pub fn forward_err(other: crate::storage::resolve::ResolveOutcome) -> Result<i32> {
    use crate::storage::resolve::ResolveOutcome;
    match other {
        ResolveOutcome::Ambiguous { query, candidates } => {
            let ids: Vec<serde_json::Value> = candidates
                .iter()
                .map(|c| serde_json::json!({"id": c.id, "title": c.title}))
                .collect();
            eprintln!(
                "{}",
                serde_json::json!({ "error": "ambiguous", "query": query, "candidates": ids })
            );
            Ok(1)
        }
        ResolveOutcome::NotFound { query } => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "not_found", "query": query })
            );
            Ok(1)
        }
        _ => unreachable!("forward_err called with Hit"),
    }
}
