//! Collections (bead P4.1): CRUD + ordered markdown export.
//!
//! DB methods live in core; this module is the command surface. Batch
//! add/remove apply atomically — first unknown id rejects the whole batch.

use std::path::Path;

use anyhow::Result;
use crate::storage::database::Database;

/// A collection with member ids in insertion order (added_at ASC).
/// Markdown export for one collection: heading + members with content.
pub fn collection_markdown(db: &Database, name: &str) -> Result<String> {
    let Some(detail) = db.collection_detail(name)? else {
        anyhow::bail!("not_found");
    };
    let mut md = format!("# {}\n\n", detail.name);
    if let Some(d) = &detail.description {
        md.push_str(&format!("{d}\n\n"));
    }
    for pid in &detail.prompts {
        if let Some(p) = db.get_prompt(pid)? {
            md.push_str(&format!("## {}\n\n{}\n\n", p.title, p.content));
        }
    }
    Ok(md)
}

/// Validate collection name against alias-safe regex shape.
pub fn valid_collection_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && Path::new(name).file_name().is_some()
}

/// CLI handler: `pst collection <verb> ...`
pub fn cmd_collection(db: &Database, args: &[String], as_json: bool) -> Result<i32> {
    match args.split_first() {
        None => {
            let all = db.collections_list()?;
            if as_json || !atty::is(atty::Stream::Stdout) {
                println!(
                    "{}",
                    serde_json::json!({
                        "collections": all.iter().map(|d| serde_json::json!({
                            "name": d.name,
                            "description": d.description,
                            "prompts": d.prompts.len(),
                        })).collect::<Vec<_>>()
                    })
                );
            } else {
                for d in &all {
                    println!("{:<24} {} prompts", d.name, d.prompts.len());
                }
            }
            Ok(0)
        }
        Some((verb, rest)) => handle_verb(db, verb, rest, as_json),
    }
}

fn handle_verb(db: &Database, verb: &str, rest: &[String], _as_json: bool) -> Result<i32> {
    match (verb, rest) {
        ("create", [name]) => create(db, name, None),
        ("create", [name, rest @ ..]) if rest.first().map(String::as_str) == Some("--desc") => {
            let desc = rest.get(1).cloned();
            create(db, name, desc.as_deref())
        }
        ("create", [name, ..]) => create(db, name, None),
        ("delete", [name]) => {
            if db.collection_delete(name)? {
                println!(
                    "{}",
                    serde_json::json!({ "deleted": true, "collection": name })
                );
                Ok(0)
            } else {
                not_found(name)
            }
        }
        ("add", [name, ids @ ..]) if !ids.is_empty() => {
            ensure_exists(db, name)?;
            match db.collection_add(name, ids) {
                Ok((added, skipped)) => {
                    println!(
                        "{}",
                        serde_json::json!({ "added": added, "skipped": skipped })
                    );
                    Ok(0)
                }
                Err(e) => not_found_member(&e.to_string()),
            }
        }
        ("remove", [name, ids @ ..]) if !ids.is_empty() => {
            let n = db.collection_remove(name, ids)?;
            println!("{}", serde_json::json!({ "removed": n }));
            Ok(0)
        }
        ("export", [name]) => export_collection(db, name, None),
        ("export", [name, flag]) if flag == "--stdout" => export_collection(db, name, None),
        ("export", [name, flag, val]) if flag == "--out" => export_collection(db, name, Some(val)),
        _ => {
            eprintln!(
                "{}",
                serde_json::json!({"error":"usage","hint":"collection [list|create|add|remove|delete|export]"})
            );
            Ok(1)
        }
    }
}

fn export_collection(db: &Database, name: &str, out: Option<&String>) -> Result<i32> {
    match crate::commands::collections::collection_markdown(db, name) {
        Ok(md) => {
            if let Some(path) = out {
                std::fs::write(path, md).map_err(|e| anyhow::anyhow!("write {path}: {e}"))?;
                println!("{}", serde_json::json!({ "exported": true, "path": path }));
            } else {
                print!("{md}");
            }
            Ok(0)
        }
        Err(e) if e.to_string() == "not_found" => not_found(name),
        Err(e) => Err(e),
    }
}

fn create(db: &Database, name: &str, desc: Option<&str>) -> Result<i32> {
    if !valid_collection_name(name) {
        eprintln!(
            "{}",
            serde_json::json!({"error":"invalid_name","message":format!("'{name}'")})
        );
        return Ok(1);
    }
    if db.collection_exists(name)? {
        eprintln!(
            "{}",
            serde_json::json!({"error":"already_exists","message":format!("collection '{name}'")})
        );
        return Ok(1);
    }
    db.collection_create(name, desc)?;
    println!("{}", serde_json::json!({ "created": name }));
    Ok(0)
}

fn ensure_exists(db: &Database, name: &str) -> Result<()> {
    if !db.collection_exists(name)? {
        anyhow::bail!("not_found:{name}");
    }
    Ok(())
}

fn not_found(name: &str) -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({ "error": "not_found", "collection": name })
    );
    Ok(1)
}

fn not_found_member(msg: &str) -> Result<i32> {
    let pid = msg.split(':').nth(1).unwrap_or("unknown");
    eprintln!(
        "{}",
        serde_json::json!({ "error": "not_found", "prompt_id": pid })
    );
    Ok(1)
}
