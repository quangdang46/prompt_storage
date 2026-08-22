//! JSONL export/import — atomic backup & restore (plan §4 rule 4, §9).
//!
//! Format:
//! - Line 1: `{"_meta":{version,count,exported_at,schema_version}}`
//! - Then one self-contained line per prompt embedding variables + aliases.
//!
//! Write path: temp file → fsync → atomic rename (Windows: remove dest first).
//! Import modes: `Merge` (upsert by id) and `Replace` (wipe + insert in ONE tx).
//! Files whose `schema_version > LATEST` are rejected.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::database::Database;
use super::schema::LATEST_SCHEMA_VERSION;
use crate::model::Prompt;

/// Metadata header (first line of every export).
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonlMeta {
    #[serde(rename = "_meta")]
    pub meta: MetaInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MetaInfo {
    pub version: String,
    pub count: usize,
    pub exported_at: String,
    pub schema_version: i32,
}

/// One prompt line: prompt fields plus embedded aliases.
#[derive(Debug, Serialize, Deserialize)]
struct PromptLine {
    #[serde(flatten)]
    prompt: Prompt,
    #[serde(default)]
    aliases: Vec<String>,
}

/// Export the entire library to `path` atomically. Returns prompt count.
pub fn export_jsonl(db: &Database, path: &Path) -> Result<usize> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let prompts = db.list_prompts_filtered(None, None, false)?;
    let count = prompts.len();

    let temp_path = path.with_extension("jsonl.tmp");
    {
        let file = File::create(&temp_path)
            .with_context(|| format!("creating temp file {:?}", temp_path))?;
        let mut writer = BufWriter::new(file);

        let meta = JsonlMeta {
            meta: MetaInfo {
                version: db
                    .get_meta("data_version")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "0".into()),
                count,
                exported_at: Utc::now().to_rfc3339(),
                schema_version: LATEST_SCHEMA_VERSION,
            },
        };
        serde_json::to_writer(&mut writer, &meta)?;
        writeln!(writer)?;

        for p in &prompts {
            let line = PromptLine {
                prompt: p.clone(),
                aliases: db.aliases_for(&p.id)?,
            };
            serde_json::to_writer(&mut writer, &line)?;
            writeln!(writer)?;
        }

        writer.flush()?;
        writer.into_inner()?.sync_all()?; // fsync before rename
    }

    // Atomic rename; Windows rename fails if destination exists.
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing existing {:?}", path))?;
    }
    fs::rename(&temp_path, path)
        .with_context(|| format!("renaming {:?} → {:?}", temp_path, path))?;

    db.set_meta("data_version", &Utc::now().to_rfc3339())?;
    Ok(count)
}

/// Import mode selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Upsert by id; existing unrelated prompts stay.
    Merge,
    /// Wipe all prompts first, then insert — all inside one transaction.
    Replace,
}

/// Import prompts from a JSONL file. Returns number of prompts applied.
pub fn import_jsonl(db: &Database, path: &Path, mode: ImportMode) -> Result<usize> {
    let file = File::open(path).with_context(|| format!("opening JSONL file {:?}", path))?;
    let reader = BufReader::new(file);

    let mut lines = Vec::new();
    let mut saw_meta = false;
    let mut line_no = 0usize;
    for line in reader.lines() {
        line_no += 1;
        let line = line.with_context(|| format!("reading line {line_no}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Only treat the FIRST non-empty line as metadata when it has "_meta".
        if !saw_meta {
            saw_meta = true;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if v.get("_meta").is_some() {
                    let meta: JsonlMeta = serde_json::from_value(v)
                        .with_context(|| format!("parsing _meta at line {line_no}"))?;
                    if meta.meta.schema_version > LATEST_SCHEMA_VERSION {
                        anyhow::bail!(
                            "schema_too_new: file schema v{}, supported v{}",
                            meta.meta.schema_version,
                            LATEST_SCHEMA_VERSION
                        );
                    }
                    continue;
                }
                // Not metadata after all — fall through as a prompt line.
            } else {
                anyhow::bail!("invalid JSON at line {line_no}");
            }
        }
        lines.push((line_no, trimmed.to_string()));
    }

    // Parse all prompt lines up-front so Replace never wipes on a bad file.
    let mut parsed = Vec::with_capacity(lines.len());
    for (line_no, text) in &lines {
        let pl: PromptLine = serde_json::from_str(text)
            .with_context(|| format!("parsing prompt at line {line_no}"))?;
        parsed.push(pl);
    }

    apply(db, mode, &parsed)?;
    Ok(parsed.len())
}

fn apply(db: &Database, mode: ImportMode, items: &[PromptLine]) -> Result<()> {
    match mode {
        ImportMode::Merge => {
            for pl in items {
                db.upsert_prompt(&pl.prompt)?;
                for alias in &pl.aliases {
                    // Alias conflicts during merge are surfaced, not swallowed.
                    db.add_alias(alias, &pl.prompt.id)?;
                }
            }
            db.set_meta("data_version", &Utc::now().to_rfc3339())?;
        }
        ImportMode::Replace => {
            // One transaction: wipe everything, then reinsert.
            let tx = db.conn().unchecked_transaction()?;

            // FTS5 virtual tables have no FK cascade — clear explicitly.
            tx.execute_batch(
                r#"
                DELETE FROM prompts_fts;
                DELETE FROM collection_prompts;
                DELETE FROM collections;
                DELETE FROM aliases;
                DELETE FROM prompt_variables;
                DELETE FROM prompt_tags;
                DELETE FROM prompts;
                "#,
            )?;

            for pl in items {
                insert_prompt_in_tx(&tx, &pl.prompt)?;
                for alias in &pl.aliases {
                    tx.execute(
                        "INSERT INTO aliases (alias, prompt_id) VALUES (?1, ?2)",
                        rusqlite::params![alias, pl.prompt.id],
                    )?;
                }
            }
            tx.commit()?;
            db.set_meta("data_version", &Utc::now().to_rfc3339())?;
        }
    }
    Ok(())
}

/// Insert a full prompt (row + tags + variables + FTS) inside an existing tx.
fn insert_prompt_in_tx(tx: &rusqlite::Transaction<'_>, p: &Prompt) -> Result<()> {
    let tags_text = p.tags.join(" ");
    tx.execute(
        r#"INSERT INTO prompts (id, title, content, description, category, tags_text,
                                version, author, difficulty, featured, source,
                                use_count, last_used_at, created_at, updated_at)
           VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,
                   ?12, ?13, COALESCE(?14, datetime('now')), datetime('now'))"#,
        rusqlite::params![
            p.id,
            p.title,
            p.content,
            p.description,
            p.category,
            tags_text,
            p.version,
            p.author,
            p.difficulty,
            p.featured as i32,
            p.source,
            p.use_count,
            p.last_used_at,
            p.created_at,
        ],
    )?;
    for tag in &p.tags {
        tx.execute(
            "INSERT INTO prompt_tags (prompt_id, tag) VALUES (?1, ?2)",
            rusqlite::params![p.id, tag],
        )?;
    }
    for var in &p.variables {
        tx.execute(
            r#"INSERT INTO prompt_variables (prompt_id, name, var_type, required, description, default_value)
               VALUES (?1,?2,?3,?4,?5,?6)"#,
            rusqlite::params![
                p.id,
                var.name,
                var.var_type.as_str(),
                var.required as i32,
                var.description,
                var.default
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO prompts_fts (id, title, description, content, tags_text)
         VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![p.id, p.title, p.description, p.content, tags_text],
    )?;
    Ok(())
}
