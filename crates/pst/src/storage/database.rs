//! Database wrapper: pragmas, tx-wrapped mutations, FTS search (plan §4).
//!
//! Operational rules:
//! - Open: parent dirs + WAL + synchronous NORMAL + busy_timeout 5s + foreign_keys ON.
//! - EVERY mutation is one transaction — including single-prompt upsert.
//! - Search: FTS5 MATCH, BM25 weights id=5,title=3,description=2,content=1,tags=2;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use super::schema::run_migrations;
use crate::model::{Prompt, PromptSummary, PromptVariable, VariableType};

/// Default on-disk database location (respects PST_HOME override at the
/// caller level; this function only joins names under a given root).
pub fn db_path_under(root: &Path) -> PathBuf {
    root.join("store.db")
}

/// Database handle. Clone-safe via new connection per use is NOT done here;
/// one `Database` owns one connection for the process lifetime.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create the database at `path`, running pending migrations.
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating db dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        Self::configure(conn)
    }

    /// Open with default location from config (caller resolves root).
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_at(&db_path_under(root))
    }

    /// In-memory database for tests — zero disk contact.
    pub fn in_memory() -> Result<Self> {
        Self::configure(Connection::open_in_memory()?)
    }

    fn configure(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    /// Access for read-only helpers that need raw SQL (doctor checks).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ------------------------------------------------------------------
    // Upsert / get / list
    // ------------------------------------------------------------------

    /// Insert or update a prompt atomically: prompts row + tags + variables +
    /// FTS row all inside ONE transaction.
    pub fn upsert_prompt(&self, prompt: &Prompt) -> Result<()> {
        let tags_text = prompt.tags.join(" ");
        let tx = self.conn.unchecked_transaction()?;
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
            params![
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

        // Tags: delete + reinsert.
        tx.execute(
            "DELETE FROM prompt_tags WHERE prompt_id = ?1",
            params![prompt.id],
        )?;
        for tag in &prompt.tags {
            tx.execute(
                "INSERT INTO prompt_tags (prompt_id, tag) VALUES (?1, ?2)",
                params![prompt.id, tag],
            )?;
        }

        // Variables: delete + reinsert.
        tx.execute(
            "DELETE FROM prompt_variables WHERE prompt_id = ?1",
            params![prompt.id],
        )?;
        for var in &prompt.variables {
            tx.execute(
                r#"INSERT INTO prompt_variables (prompt_id, name, var_type, required, description, default_value)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    prompt.id,
                    var.name,
                    var.var_type.as_str(),
                    var.required as i32,
                    var.description,
                    var.default,
                ],
            )?;
        }

        // FTS row: delete + reinsert (shared helper so doctor rebuild and
        // normal writes can never drift apart).
        fts_sync(
            &tx,
            &prompt.id,
            &prompt.title,
            &prompt.description,
            &prompt.content,
            &tags_text,
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Fetch one prompt by exact canonical id (no resolution here — see resolve.rs).
    pub fn get_prompt(&self, id: &str) -> Result<Option<Prompt>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, title, content, description, category, version, author,
                      difficulty, featured, source, use_count, last_used_at,
                      created_at, updated_at
               FROM prompts WHERE id = ?1"#,
        )?;
        let mut rows = stmt.query_map(params![id], map_prompt_row)?;
        match rows.next() {
            None => Ok(None),
            Some(row) => {
                let mut prompt = row?;
                self.load_children(&mut prompt)?;
                Ok(Some(prompt))
            }
        }
    }

    /// List prompts with optional filters, ordered by title.
    pub fn list_prompts_filtered(
        &self,
        category: Option<&str>,
        tag: Option<&str>,
        featured_only: bool,
    ) -> Result<Vec<Prompt>> {
        let mut conditions: Vec<&str> = Vec::new();
        if category.is_some() {
            conditions.push("p.category = ?1");
        }
        if tag.is_some() {
            if category.is_some() {
                conditions.push("p.id IN (SELECT prompt_id FROM prompt_tags WHERE tag = ?2)");
            } else {
                conditions.push("p.id IN (SELECT prompt_id FROM prompt_tags WHERE tag = ?1)");
            }
        }
        if featured_only {
            conditions.push("p.featured = 1");
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            r#"SELECT p.id, p.title, p.content, p.description, p.category, p.version, p.author,
                      p.difficulty, p.featured, p.source, p.use_count, p.last_used_at,
                      p.created_at, p.updated_at
               FROM prompts p {where_clause} ORDER BY p.title"#
        );
        let cat = category.map(str::to_string);
        let tagv = tag.map(str::to_string);
        // rusqlite named-parameter binding requires the parameter to appear
        // in the SQL text; when a filter is absent its name never occurs, so
        // we fall back to positional binding via a tiny wrapper.
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<Prompt> = if cat.is_some() && tagv.is_some() {
            stmt.query_and_then(
                rusqlite::params![cat.as_deref(), tagv.as_deref()],
                Self::map_prompt_with_children(self),
            )?
            .collect::<Result<Vec<_>, _>>()?
        } else if let Some(c) = cat.as_deref() {
            stmt.query_and_then(rusqlite::params![c], Self::map_prompt_with_children(self))?
                .collect::<Result<Vec<_>, _>>()?
        } else if let Some(t) = tagv.as_deref() {
            stmt.query_and_then(rusqlite::params![t], Self::map_prompt_with_children(self))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_and_then([], Self::map_prompt_with_children(self))?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// Summaries only (no children) — cheap listing for `pst list`.
    pub fn list_summaries(&self) -> Result<Vec<PromptSummary>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT p.id, p.title, p.description, p.category, p.use_count,
                      COALESCE((SELECT GROUP_CONCAT(tag, char(31)) FROM prompt_tags t WHERE t.prompt_id = p.id), '')
               FROM prompts p ORDER BY p.title"#,
        )?;
        let rows = stmt.query_map([], |row| {
            let tags_blob: String = row.get(5)?;
            Ok(PromptSummary {
                id: row.get(0)?,

                title: row.get(1)?,
                description: row.get(2)?,
                category: row.get(3)?,
                use_count: row.get(4)?,
                tags: if tags_blob.is_empty() {
                    Vec::new()
                } else {
                    tags_blob.split('\u{1f}').map(str::to_string).collect()
                },
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Closure factory: hydrate a Prompt row plus its tags/variables.
    fn map_prompt_with_children(
        db: &Database,
    ) -> impl Fn(&rusqlite::Row<'_>) -> anyhow::Result<Prompt> + '_ {
        move |row| {
            let mut p = map_prompt_row(row)?;
            db.load_children(&mut p)?;
            Ok(p)
        }
    }

    // ------------------------------------------------------------------
    // Taxonomy counts
    // ------------------------------------------------------------------

    pub fn category_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT category, COUNT(*) FROM prompts WHERE category IS NOT NULL GROUP BY category ORDER BY category",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        collect(rows)
    }

    pub fn tag_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag, COUNT(*) FROM prompt_tags GROUP BY tag ORDER BY COUNT(*) DESC, tag",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        collect(rows)
    }

    pub fn prompt_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))?)
    }

    // ------------------------------------------------------------------
    // Search (FTS5 BM25)
    // ------------------------------------------------------------------

    /// Full-text search with BM25 ranking. Weights: id=5 title=3 desc=2 content=1 tags=2.
    /// Returns (prompt, score_descending). Scores are negated because BM25 yields
    /// negative values (lower == better).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(Prompt, f64)>> {
        let escaped = escape_fts_query(query);
        let mut stmt = self.conn.prepare(
            r#"
            SELECT p.id, p.title, p.content, p.description, p.category, p.version, p.author,
                   p.difficulty, p.featured, p.source, p.use_count, p.last_used_at,
                   p.created_at, p.updated_at,
                   bm25(prompts_fts, 5.0, 3.0, 2.0, 1.0, 2.0) AS score
            FROM prompts_fts f
            JOIN prompts p ON p.id = f.id
            WHERE prompts_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;
        let limit_i = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![escaped, limit_i], |row| {
            let score: f64 = row.get(14)?;
            Ok((map_prompt_row(row)?, -score))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (mut p, score) = r?;
            self.load_children(&mut p)?;
            out.push((p, score));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Aliases
    // ------------------------------------------------------------------

    /// Add an alias after enforcing collision invariants (plan §5):
    /// alias must not equal ANY canonical id (exact OR case-insensitive),
    /// and must be unique among aliases (NOCASE). Check-and-insert share
    /// one transaction → race-free under WAL single-writer.
    pub fn add_alias(&self, alias: &str, prompt_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let id_hit: bool = tx
            .query_row(
                "SELECT COUNT(*) > 0 FROM prompts WHERE id = ?1 COLLATE NOCASE",
                params![alias],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if id_hit {
            anyhow::bail!("id_conflict");
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT prompt_id FROM aliases WHERE alias = ?1",
                params![alias],
                |r| r.get(0),
            )
            .ok();
        if let Some(owner) = existing {
            if owner != prompt_id {
                anyhow::bail!("alias_conflict");
            } else {
                return Ok(()); // idempotent re-add of same mapping
            }
        }
        let target_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) > 0 FROM prompts WHERE id = ?1",
                params![prompt_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !target_exists {
            anyhow::bail!("not_found");
        }
        tx.execute(
            "INSERT INTO aliases (alias, prompt_id) VALUES (?1, ?2)",
            params![alias, prompt_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_alias(&self, alias: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM aliases WHERE alias = ?1", params![alias])?;
        Ok(n > 0)
    }

    pub fn lookup_alias(&self, alias: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT prompt_id FROM aliases WHERE alias = ?1",
                params![alias],
                |r| r.get(0),
            )
            .ok())
    }

    pub fn aliases_for(&self, prompt_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT alias FROM aliases WHERE prompt_id = ?1 ORDER BY alias")?;
        let rows = stmt.query_map(params![prompt_id], |r| r.get(0))?;
        collect(rows)
    }

    // ------------------------------------------------------------------
    // Delete + usage tracking
    // ------------------------------------------------------------------

    /// Delete a prompt plus its FTS row atomically. FK cascade removes
    /// tags/variables/aliases; FTS5 virtual tables have no FK support, so
    /// the index row is removed in the same transaction.
    pub fn delete_prompt(&self, id: &str) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM prompts_fts WHERE id = ?1", params![id])?;
        let n = tx.execute("DELETE FROM prompts WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Bump use_count + set last_used_at for prefix/fuzzy hits (one tx).
    pub fn bump_usage(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE prompts SET use_count = use_count + 1, last_used_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Meta
    // ------------------------------------------------------------------

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .ok())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn load_children(&self, prompt: &mut Prompt) -> Result<()> {
        prompt.tags = {
            let mut stmt = self
                .conn
                .prepare("SELECT tag FROM prompt_tags WHERE prompt_id = ?1")?;
            let rows = stmt.query_map(params![prompt.id], |r| r.get(0))?;
            collect(rows)?
        };
        prompt.variables = {
            let mut stmt = self.conn.prepare(
                "SELECT name, var_type, required, description, default_value
                 FROM prompt_variables WHERE prompt_id = ?1 ORDER BY name",
            )?;
            let rows = stmt.query_map(params![prompt.id], |r| {
                let vt: String = r.get(1)?;
                Ok(PromptVariable {
                    name: r.get(0)?,
                    var_type: VariableType::from_str_opt(&vt).unwrap_or_default(),
                    required: r.get::<_, i32>(2)? != 0,
                    description: r.get(3)?,
                    default: r.get(4)?,
                })
            })?;
            collect(rows)?
        };
        Ok(())
    }
}

/// Keep the FTS index in sync for one prompt id (delete + insert).
/// Shared by upsert and doctor --fix rebuild.
fn fts_sync(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    title: &str,
    description: &Option<String>,
    content: &str,
    tags_text: &str,
) -> Result<()> {
    tx.execute("DELETE FROM prompts_fts WHERE id = ?1", params![id])?;
    tx.execute(
        "INSERT INTO prompts_fts (id, title, description, content, tags_text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, title, description, content, tags_text],
    )?;
    Ok(())
}

fn map_prompt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Prompt> {
    Ok(Prompt {
        id: row.get(0)?,
        title: row.get(1)?,
        content: row.get(2)?,
        description: row.get(3)?,
        category: row.get(4)?,
        tags: Vec::new(),
        variables: Vec::new(),
        version: row.get(5)?,
        author: row.get(6)?,
        difficulty: row.get(7)?,
        featured: row.get::<_, i32>(8)? != 0,
        source: row.get(9)?,
        use_count: row.get(10)?,
        last_used_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>>
where
    T: Sized,
{
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Escape user query text before handing it to FTS5 MATCH so special
/// characters never panic the parser.
pub fn escape_fts_query(query: &str) -> String {
    // Wrap each token in double quotes; escape embedded quotes. Tokens are
    // OR-ed: any token matching keeps the prompt discoverable (BM25 ranks
    // multi-token matches higher automatically). Trailing `*` gives
    // per-token prefix matching so "sec" still finds "security".
    query
        .split_whitespace()
        .map(|tok| format!("\"{}\"*", tok.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}
