//! Command modules — presentation logic only (plan §3 architecture rule).
pub mod core;
pub mod discovery;

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
        ResolveOutcome::Hit {
            id, title, ..
        } => {
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
