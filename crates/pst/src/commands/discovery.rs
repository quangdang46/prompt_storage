//! Discovery commands (bead P2.4): list, categories, tags.
//!
//! All auto-JSON when stdout is not a TTY per §6 enumeration rule.

use anyhow::Result;

use crate::model::Prompt;
use crate::storage::database::Database;

/// Bold-wrap for human output; only called on the TTY path after the
/// caller already gated on `output.color`/`--no-color`/`NO_COLOR`.
pub fn bold(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// `pst list [--category C] [--tag t] [--featured] [--limit N]`
pub fn cmd_list(
    db: &Database,
    category: Option<&str>,
    tag: Option<&str>,
    featured: bool,
    limit: Option<usize>,
    as_json: bool,
    color: bool,
) -> Result<i32> {
    // Summaries only: no content blobs, no per-row children queries.
    let prompts = db.list_summaries_filtered(category, tag, featured)?;
    let total = prompts.len();
    let taken: Vec<_> = match limit {
        Some(n) => prompts.into_iter().take(n).collect(),
        None => prompts,
    };

    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = taken
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "title": p.title,
                    "description": p.description,
                    "category": p.category,
                    "tags": p.tags,
                    "use_count": p.use_count,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "prompts": rows, "count": taken.len(), "total": total })
        );
    } else {
        for p in &taken {
            let tags = if p.tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", p.tags.join(","))
            };
            println!(
                "{:<28} {}{}",
                bold(&p.id, color),
                bold(&p.title, color),
                tags
            );
        }
        println!("\n{} of {total} prompts", taken.len());
    }
    Ok(0)
}

/// `pst categories`
pub fn cmd_categories(db: &Database, as_json: bool, color: bool) -> Result<i32> {
    let counts = db.category_counts()?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = counts
            .iter()
            .map(|(c, n)| serde_json::json!({"category": c, "count": n}))
            .collect();
        println!("{}", serde_json::json!({ "categories": rows }));
    } else {
        for (c, n) in &counts {
            println!("{:<24} {n}", bold(c, color));
        }
    }
    Ok(0)
}

/// `pst tags`
pub fn cmd_tags(db: &Database, as_json: bool, color: bool) -> Result<i32> {
    let counts = db.tag_counts()?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = counts
            .iter()
            .map(|(t, n)| serde_json::json!({"tag": t, "count": n}))
            .collect();
        println!("{}", serde_json::json!({ "tags": rows }));
    } else {
        for (t, n) in &counts {
            println!("{:<24} {n}", bold(t, color));
        }
    }
    Ok(0)
}

/// `pst search <query> [--limit N]` — FTS BM25 ranked results.
pub fn cmd_search(
    db: &Database,
    query: &str,
    limit: usize,
    as_json: bool,
    color: bool,
) -> Result<i32> {
    if query.trim().is_empty() {
        eprintln!("{}", serde_json::json!({"error":"empty_query"}));
        return Ok(1);
    }
    // --limit 0 means "no results" — honor it literally.
    if limit == 0 {
        if as_json || !atty::is(atty::Stream::Stdout) {
            println!(
                "{}",
                serde_json::json!({ "results": [], "query": query, "count": 0 })
            );
        }
        return Ok(0);
    }
    let hits = db.search(query, limit)?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = hits
            .iter()
            .map(|(p, score)| {
                serde_json::json!({
                    "id": p.id,
                    "title": p.title,
                    "description": p.description,
                    "score": score,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "results": rows, "query": query, "count": rows.len() })
        );
    } else {
        for (p, score) in &hits {
            println!(
                "{:<28} {:<40} score={:.3}",
                bold(&p.id, color),
                bold(&p.title, color),
                score
            );
        }
        if hits.is_empty() {
            println!("no matches");
        }
    }
    Ok(0)
}

/// Which prompt fields matched the query terms — the "reason" payload.
fn match_reason(p: &Prompt, terms: &[String]) -> String {
    let mut fields = Vec::new();
    let title = p.title.to_lowercase();
    let desc = p.description.as_deref().unwrap_or("").to_lowercase();
    let tags = p.tags.join(" ").to_lowercase();
    let content = p.content.to_lowercase();
    if terms.iter().any(|t| p.id.to_lowercase().contains(t)) {
        fields.push("id");
    }
    if terms.iter().any(|t| title.contains(t)) {
        fields.push("title");
    }
    if terms.iter().any(|t| desc.contains(t)) {
        fields.push("description");
    }
    if terms.iter().any(|t| tags.contains(t)) {
        fields.push("tags");
    }
    if terms.iter().any(|t| content.contains(t)) {
        fields.push("content");
    }
    if fields.is_empty() {
        // FTS matched via tokenization we didn't reproduce — still honest.
        fields.push("fts");
    }
    fields.join("+")
}

/// `pst suggest <task> [--limit N]` — ranked recommendations with reasons.
/// Default limit comes from config `suggest.default_limit` (wired in main).
pub fn cmd_suggest(
    db: &Database,
    query: &str,
    limit: Option<usize>,
    default_limit: usize,
    as_json: bool,
    color: bool,
) -> Result<i32> {
    if query.trim().is_empty() {
        eprintln!("{}", serde_json::json!({"error":"empty_query"}));
        return Ok(1);
    }
    let lim = limit.unwrap_or(default_limit);
    if lim == 0 {
        if as_json || !atty::is(atty::Stream::Stdout) {
            println!(
                "{}",
                serde_json::json!({ "suggestions": [], "query": query, "count": 0 })
            );
        }
        return Ok(0);
    }
    let hits = db.search(query, lim)?;
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let rows: Vec<serde_json::Value> = hits
        .iter()
        .map(|(p, score)| {
            serde_json::json!({
                "id": p.id,
                "title": p.title,
                "score": score,
                "reason": match_reason(p, &terms),
            })
        })
        .collect();
    if as_json || !atty::is(atty::Stream::Stdout) {
        println!(
            "{}",
            serde_json::json!({ "suggestions": rows, "query": query, "count": rows.len() })
        );
    } else {
        for (p, score) in &hits {
            println!(
                "{:<28} {:<40} score={:.3} ({})",
                bold(&p.id, color),
                bold(&p.title, color),
                score,
                match_reason(p, &terms)
            );
        }
        if hits.is_empty() {
            println!("no suggestions");
        }
    }
    Ok(0)
}
