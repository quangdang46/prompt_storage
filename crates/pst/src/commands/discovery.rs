//! Discovery commands (bead P2.4): list, categories, tags.
//!
//! All auto-JSON when stdout is not a TTY per §6 enumeration rule.

use anyhow::Result;

use crate::storage::database::Database;

/// `pst list [--category C] [--tag t] [--featured] [--limit N]`
pub fn cmd_list(
    db: &Database,
    category: Option<&str>,
    tag: Option<&str>,
    featured: bool,
    limit: Option<usize>,
    as_json: bool,
) -> Result<i32> {
    let prompts = db.list_prompts_filtered(category, tag, featured)?;
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
            println!("{:<28} {}{}", p.id, p.title, tags);
        }
        println!("\n{} of {total} prompts", taken.len());
    }
    Ok(0)
}

/// `pst categories`
pub fn cmd_categories(db: &Database, as_json: bool) -> Result<i32> {
    let counts = db.category_counts()?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = counts
            .iter()
            .map(|(c, n)| serde_json::json!({"category": c, "count": n}))
            .collect();
        println!("{}", serde_json::json!({ "categories": rows }));
    } else {
        for (c, n) in &counts {
            println!("{c:<24} {n}");
        }
    }
    Ok(0)
}

/// `pst tags`
pub fn cmd_tags(db: &Database, as_json: bool) -> Result<i32> {
    let counts = db.tag_counts()?;
    if as_json || !atty::is(atty::Stream::Stdout) {
        let rows: Vec<serde_json::Value> = counts
            .iter()
            .map(|(t, n)| serde_json::json!({"tag": t, "count": n}))
            .collect();
        println!("{}", serde_json::json!({ "tags": rows }));
    } else {
        for (t, n) in &counts {
            println!("{t:<24} {n}");
        }
    }
    Ok(0)
}

/// `pst search <query> [--limit N]` — FTS BM25 ranked results.
pub fn cmd_search(db: &Database, query: &str, limit: usize, as_json: bool) -> Result<i32> {
    if query.trim().is_empty() {
        eprintln!("{}", serde_json::json!({"error":"empty_query"}));
        return Ok(1);
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
            println!("{:<28} {:<40} score={:.3}", p.id, p.title, score);
        }
        if hits.is_empty() {
            println!("no matches");
        }
    }
    Ok(0)
}
