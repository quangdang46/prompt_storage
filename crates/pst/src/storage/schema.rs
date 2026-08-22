//! Schema definition and forward-only migrations (plan §4).
//!
//! `PRAGMA user_version` is the single source of truth for schema versioning.
//! Migrations are ordered and append-only: entry `i` upgrades version
//! `i-1 -> i`. Databases written by a NEWER pst are rejected with
//! `schema_too_new` instead of being corrupted (downgrade guard).

use rusqlite::Connection;

pub const LATEST_SCHEMA_VERSION: i32 = 1;

/// Ordered, append-only. Entry `i` upgrades version `i-1 -> i`.
/// NEVER edit an existing entry — add a new one at the end.
pub const MIGRATIONS: &[&str] = &[
    // 0 -> 1: initial schema
    r#"
CREATE TABLE IF NOT EXISTS prompts (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    content       TEXT NOT NULL,
    description   TEXT,
    category      TEXT,
    tags_text     TEXT,
    version       TEXT,
    author        TEXT,
    difficulty    TEXT,
    featured      INTEGER NOT NULL DEFAULT 0,
    source        TEXT NOT NULL DEFAULT 'manual',
    use_count     INTEGER NOT NULL DEFAULT 0,
    last_used_at  TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS aliases (
    alias     TEXT PRIMARY KEY COLLATE NOCASE,
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS prompt_tags (
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    tag       TEXT NOT NULL,
    PRIMARY KEY (prompt_id, tag)
);

CREATE TABLE IF NOT EXISTS prompt_variables (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    var_type TEXT NOT NULL DEFAULT 'text',
    required INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    default_value TEXT
);

CREATE TABLE IF NOT EXISTS collections (
    name TEXT PRIMARY KEY COLLATE NOCASE,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS collection_prompts (
    collection_name TEXT NOT NULL REFERENCES collections(name) ON DELETE CASCADE,
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (collection_name, prompt_id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    id, title, description, content, tags_text
);

CREATE INDEX IF NOT EXISTS idx_prompts_category ON prompts(category);
CREATE INDEX IF NOT EXISTS idx_prompt_tags_tag  ON prompt_tags(tag);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#,
];

/// Error type for migration failures.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "database was written by a newer pst (schema v{db_version}, supported v{supported}) — upgrade pst"
    )]
    Downgrade { db_version: i32, supported: i32 },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

/// Current schema version of an opened database.
pub fn current_version(conn: &Connection) -> Result<i32, MigrationError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Run all pending migrations inside individual transactions.
/// Idempotent: opening an already-migrated database is a no-op.
pub fn run_migrations(conn: &Connection) -> Result<(), MigrationError> {
    let current = current_version(conn)?;
    if current > LATEST_SCHEMA_VERSION {
        return Err(MigrationError::Downgrade {
            db_version: current,
            supported: LATEST_SCHEMA_VERSION,
        });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i32;
        if current < target {
            conn.execute_batch(&format!(
                "BEGIN; {sql}; PRAGMA user_version = {target}; COMMIT;"
            ))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_gets_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn reopen_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // second open: no-op
        assert_eq!(current_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn downgrade_guard_rejects_newer_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 5)
            .unwrap();
        let err = run_migrations(&conn).unwrap_err();
        assert!(matches!(err, MigrationError::Downgrade { .. }));
    }

    #[test]
    fn migration_creates_expected_tables() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let expected = [
            "prompts",
            "aliases",
            "prompt_tags",
            "prompt_variables",
            "collections",
            "collection_prompts",
            "prompts_fts",
            "meta",
        ];
        for table in expected {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table/view {table} missing after migration");
        }
    }

    #[test]
    fn foreign_keys_cascade_on_alias_delete() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO prompts (id, title, content) VALUES ('demo', 'Demo', 'x');
             INSERT INTO aliases (alias, prompt_id) VALUES ('d', 'demo');",
        )
        .unwrap();
        conn.execute("DELETE FROM prompts WHERE id = 'demo'", [])
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM aliases", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "aliases should cascade-delete with their prompt");
    }
}
