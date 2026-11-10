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
    let prompts = db.list_prompts_filtered(None, None, false)?;
    export_jsonl_prompts(db, path, &prompts)
}

/// Export a caller-selected prompt set to `path` atomically.
/// `prompts` must already be hydrated `Prompt` rows (ids/aliases resolved
/// by the caller). Returns the number of prompts written.
pub fn export_jsonl_prompts(db: &Database, path: &Path, prompts: &[Prompt]) -> Result<usize> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let count = prompts.len();

    // Unique temp name (pid + counter) so concurrent exports can't collide,
    // and it's removed on failure so no .tmp litter remains.
    let temp_path = path.with_extension(format!("jsonl.tmp{}", std::process::id()));
    let write_result = (|| -> Result<()> {
        let file = File::create(&temp_path)
            .with_context(|| format!("creating temp file {:?}", temp_path))?;
        let mut writer = BufWriter::new(file);
        write_jsonl(db, prompts, &mut writer)?;
        writer.flush()?;
        writer.into_inner()?.sync_all()?; // fsync before rename
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&temp_path); // no .tmp litter on failure
        return Err(e);
    }

    // Atomic rename; Windows rename fails if destination exists.
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing existing {:?}", path))?;
    }
    if let Err(e) = fs::rename(&temp_path, path)
        .with_context(|| format!("renaming {:?} → {:?}", temp_path, path))
    {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }

    db.set_meta("data_version", &Utc::now().to_rfc3339())?;
    Ok(count)
}

/// Write the JSONL document (meta header + one line per prompt) to any
/// writer — used for `--stdout` export where no temp file is involved.
pub fn export_jsonl_to<W: Write>(db: &Database, prompts: &[Prompt], w: &mut W) -> Result<usize> {
    write_jsonl(db, prompts, w)?;
    w.flush()?;
    Ok(prompts.len())
}

/// Shared serializer: `_meta` header line, then one self-contained
/// prompt line (variables + aliases embedded) per prompt.
fn write_jsonl(db: &Database, prompts: &[Prompt], w: &mut impl Write) -> Result<()> {
    let meta = JsonlMeta {
        meta: MetaInfo {
            version: db
                .get_meta("data_version")
                .ok()
                .flatten()
                .unwrap_or_else(|| "0".into()),
            count: prompts.len(),
            exported_at: Utc::now().to_rfc3339(),
            schema_version: LATEST_SCHEMA_VERSION,
        },
    };
    serde_json::to_writer(&mut *w, &meta)?;
    writeln!(w)?;

    for p in prompts {
        let line = PromptLine {
            prompt: p.clone(),
            aliases: db.aliases_for(&p.id)?,
        };
        serde_json::to_writer(&mut *w, &line)?;
        writeln!(w)?;
    }
    Ok(())
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
        // Only treat the FIRST non-empty line as metadata when it has "_meta"
        // and is NOT itself a prompt line (a prompt with a stray top-level
        // "_meta" key must not be silently dropped).
        if !saw_meta {
            saw_meta = true;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if v.get("_meta").is_some() && v.get("id").is_none() {
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
    // Every line is validated with the SAME rules `pst new` enforces:
    // canonical kebab-case id and no NUL bytes in stored text.
    let mut parsed = Vec::with_capacity(lines.len());
    for (line_no, text) in &lines {
        let pl: PromptLine = serde_json::from_str(text)
            .with_context(|| format!("parsing prompt at line {line_no}"))?;
        validate_imported_prompt(&pl.prompt)
            .with_context(|| format!("invalid prompt at line {line_no}"))?;
        for alias in &pl.aliases {
            validate_alias(alias).with_context(|| format!("invalid alias at line {line_no}"))?;
        }
        parsed.push(pl);
    }

    apply(db, mode, &parsed)?;
    Ok(parsed.len())
}

fn apply(db: &Database, mode: ImportMode, items: &[PromptLine]) -> Result<()> {
    match mode {
        ImportMode::Merge => {
            // ONE transaction: a mid-file failure (alias conflict, shadowed
            // id) must not leave a half-imported library behind.
            let tx = db.conn().unchecked_transaction()?;
            for pl in items {
                // A new canonical id that equals an existing alias would
                // silently shadow that alias forever (exact-id wins in
                // resolve). Reject instead — same rule `new` enforces.
                let shadowed: Option<String> = tx
                    .query_row(
                        "SELECT prompt_id FROM aliases WHERE alias = ?1",
                        rusqlite::params![pl.prompt.id],
                        |r| r.get(0),
                    )
                    .ok();
                if shadowed.is_some() {
                    anyhow::bail!(
                        "alias_conflict: id '{}' collides with an existing alias",
                        pl.prompt.id
                    );
                }
                upsert_prompt_in_tx(&tx, &pl.prompt)?;
                for alias in &pl.aliases {
                    insert_alias_checked(&tx, alias, &pl.prompt.id)?;
                }
            }
            tx.commit()?;
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

            // Pass 1: all prompts. Pass 2: aliases through the SAME
            // validation add_alias enforces (the old raw INSERT bypassed
            // id_conflict/alias_conflict checks entirely).
            for pl in items {
                insert_prompt_in_tx(&tx, &pl.prompt)?;
            }
            for pl in items {
                for alias in &pl.aliases {
                    insert_alias_checked(&tx, alias, &pl.prompt.id)?;
                }
            }
            tx.commit()?;
            db.set_meta("data_version", &Utc::now().to_rfc3339())?;
        }
    }
    Ok(())
}

/// Import-side validation: identical id rules to `pst new`, plus a ban on
/// NUL bytes in any stored text field (they corrupt JSONL round-trips).
fn validate_imported_prompt(p: &Prompt) -> Result<()> {
    if !crate::model::is_canonical_id(&p.id) {
        anyhow::bail!("invalid_id: '{}'", p.id);
    }
    for (field, value) in [
        ("title", p.title.as_str()),
        ("content", p.content.as_str()),
        ("description", p.description.as_deref().unwrap_or("")),
        ("category", p.category.as_deref().unwrap_or("")),
    ] {
        if value.contains('\0') {
            anyhow::bail!("invalid_content: NUL byte in {field}");
        }
    }
    Ok(())
}

/// Alias shape rules — shared implementation lives in database.rs.
fn validate_alias(alias: &str) -> Result<()> {
    super::database::validate_alias_shape(alias)
}

/// Insert-or-update a prompt inside an existing transaction — mirrors
/// `Database::upsert_prompt` exactly (same SQL, same tag/var/FTS handling).
fn upsert_prompt_in_tx(tx: &rusqlite::Transaction<'_>, prompt: &Prompt) -> Result<()> {
    let tags_text = prompt.tags.join(" ");
    tx.execute(
        r#"
        INSERT INTO prompts (id, title, content, description, category, tags_text,
                             version, author, difficulty, featured, source,
                             use_count, last_used_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                CASE WHEN ?12 > 0 THEN ?12
                     ELSE COALESCE((SELECT use_count FROM prompts WHERE id = ?1), 0) END,
                CASE WHEN ?13 IS NOT NULL THEN ?13
                     ELSE (SELECT last_used_at FROM prompts WHERE id = ?1) END)
        ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            content = excluded.content,
            description = excluded.description,
            category = excluded.category,
            tags_text = excluded.tags_text,
            version = excluded.version,
            author = excluded.author,
            difficulty = excluded.difficulty,
            featured = excluded.featured,
            source = excluded.source,
            updated_at = datetime('now')
        "#,
        rusqlite::params![
            prompt.id,
            prompt.title,
            prompt.content,
            prompt.description,
            prompt.category,
            tags_text,
            prompt.version,
            prompt.author,
            prompt.difficulty,
            prompt.featured as i32,
            prompt.source,
            prompt.use_count,
            prompt.last_used_at,
        ],
    )
    .context("upserting prompt row")?;

    tx.execute(
        "DELETE FROM prompt_tags WHERE prompt_id = ?1",
        rusqlite::params![prompt.id],
    )?;
    for tag in &prompt.tags {
        tx.execute(
            "INSERT INTO prompt_tags (prompt_id, tag) VALUES (?1, ?2)",
            rusqlite::params![prompt.id, tag],
        )?;
    }

    tx.execute(
        "DELETE FROM prompt_variables WHERE prompt_id = ?1",
        rusqlite::params![prompt.id],
    )?;
    for var in &prompt.variables {
        tx.execute(
            r#"INSERT INTO prompt_variables (prompt_id, name, var_type, required, description, default_value)
               VALUES (?1,?2,?3,?4,?5,?6)"#,
            rusqlite::params![
                prompt.id,
                var.name,
                var.var_type.as_str(),
                var.required as i32,
                var.description,
                var.default,
            ],
        )?;
    }

    super::database::fts_sync(
        tx,
        &prompt.id,
        &prompt.title,
        &prompt.description,
        &prompt.content,
        &tags_text,
    )?;
    Ok(())
}

/// Alias insert with the SAME collision rules as `Database::add_alias`,
/// runnable inside an existing transaction (replace-mode import).
/// Delegates to the shared implementation so rules can never drift.
fn insert_alias_checked(
    tx: &rusqlite::Transaction<'_>,
    alias: &str,
    prompt_id: &str,
) -> Result<()> {
    super::database::add_alias_in_tx(tx, alias, prompt_id)
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
