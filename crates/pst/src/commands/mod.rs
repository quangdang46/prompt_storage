//! Command modules — presentation logic only (plan §3 architecture rule).
pub mod collections;
pub mod core;
pub mod discovery;
pub mod export;
pub mod system;

use crate::storage::database::Database;
use anyhow::Result;

/// Shared render pipeline for `render` and `copy`:
/// resolve → hydrate → build_values → missing-required handling → render.
/// Returns the rendered string, or emits the contract error and returns
/// the exit code via `Err(code)`.
fn render_prompt(
    db: &Database,
    query: &str,
    prescan_vars: &[(String, String)],
    fill: bool,
) -> std::result::Result<(crate::model::Prompt, String), i32> {
    use crate::storage::resolve::{ResolveOutcome, resolve};

    let outcome = resolve(db, query).map_err(|e| {
        eprintln!(
            "{}",
            serde_json::json!({"error":"internal","message":e.to_string()})
        );
        1
    })?;
    let id = match outcome {
        ResolveOutcome::Hit { id, .. } => id,
        other => return Err(forward_err(other).unwrap_or(1)),
    };
    let prompt = match db.get_prompt(&id) {
        Ok(Some(p)) => p,
        Ok(None) => return Err(1),
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"error":"internal","message":e.to_string()})
            );
            return Err(1);
        }
    };

    let cwd = std::env::current_dir().unwrap_or_default();
    let mut values = match crate::render::build_values(&prompt, prescan_vars, &cwd) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "variable_error", "message": e.to_string() })
            );
            return Err(1);
        }
    };

    // Missing REQUIRED variables (declared with '!' or required:true and
    // no default/explicit/dynamic value).
    let missing: Vec<String> = prompt
        .variables
        .iter()
        .filter(|v| v.required && !values.contains_key(&v.name))
        .map(|v| v.name.clone())
        .collect();
    if !missing.is_empty() {
        if !fill {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "missing_variables",
                    "missing": missing,
                    "hint": "pass --VAR=value or use --fill on a TTY"
                })
            );
            return Err(1);
        }
        if !atty::is(atty::Stream::Stdin) {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "tty_required",
                    "missing": missing,
                    "hint": "non-interactive: pass explicit --VAR=value flags"
                })
            );
            return Err(1);
        }
        // Interactive fill: prompt on stderr, read one line per var.
        use std::io::Write;
        for name in &missing {
            eprint!("{name}: ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                eprintln!(
                    "{}",
                    serde_json::json!({"error":"cancelled","message":"stdin closed during --fill"})
                );
                return Err(130);
            }
            values.insert(
                name.clone(),
                line.trim_end_matches(['\n', '\r']).to_string(),
            );
        }
    }

    // Case-mismatch nudge: a provided --var that differs only by case from
    // a declared name is almost certainly a typo (vars are case-sensitive).
    for (name, _) in prescan_vars {
        if let Some(declared) = prompt
            .variables
            .iter()
            .map(|v| v.name.as_str())
            .find(|d| d.eq_ignore_ascii_case(name) && *d != name)
        {
            eprintln!(
                "{}",
                serde_json::json!({
                    "warning": "case_mismatch",
                    "provided": name,
                    "declared": declared,
                    "hint": "variable names are case-sensitive"
                })
            );
        }
    }

    let (rendered, _) = crate::render::render_content(&prompt.content, &values);
    Ok((prompt, rendered))
}

/// `pst copy <query> [--fill] [--VAR=value...]` — resolve, render with the
/// same pipeline as `render` (defaults always applied), copy to clipboard.
/// stdout stays data-only per contract §6.
pub fn copy_cmd(
    db: &Database,
    query: &str,
    fill: bool,
    as_json: bool,
    prescan_vars: &[(String, String)],
) -> Result<i32> {
    let (prompt, final_text) = match render_prompt(db, query, prescan_vars, fill) {
        Ok(r) => r,
        Err(code) => {
            // Non-Hit resolve outcomes already emitted their payload inside
            // render_prompt via forward_err — but forward_err returns Ok(1)
            // with the payload printed; Err(1) here means "already reported".
            return Ok(code);
        }
    };
    let id = prompt.id.clone();
    let title = prompt.title.clone();

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

/// `pst render <query> [--VAR=value...] [--fill]` — print rendered prompt
/// to stdout. Shares the copy_cmd pipeline: defaults always apply,
/// missing required vars → missing_variables / tty_required / interactive
/// --fill on a TTY.
pub fn render_cmd(
    db: &Database,
    query: &str,
    as_json: bool,
    prescan_vars: &[(String, String)],
    fill: bool,
) -> Result<i32> {
    match render_prompt(db, query, prescan_vars, fill) {
        Ok((prompt, rendered)) => {
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
        Err(code) => Ok(code),
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
