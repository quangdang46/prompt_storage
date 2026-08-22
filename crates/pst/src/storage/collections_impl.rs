//! Collection DB methods (bead P4.1) — impl block on Database.

use anyhow::Result;
use rusqlite::params;
use serde::Serialize;

use super::database::Database;

#[derive(Debug, Serialize)]
pub struct CollectionDetail {
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub prompts: Vec<String>,
}

impl Database {
    pub fn collection_create(&self, name: &str, description: Option<&str>) -> Result<()> {
        self.conn().execute(
            "INSERT INTO collections (name, description) VALUES (?1, ?2)",
            params![name, description],
        )?;
        Ok(())
    }

    pub fn collection_delete(&self, name: &str) -> Result<bool> {
        let n = self
            .conn()
            .execute("DELETE FROM collections WHERE name = ?1", params![name])?;
        Ok(n > 0)
    }

    pub fn collection_exists(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT COUNT(*) > 0 FROM collections WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .unwrap_or(false))
    }

    /// Add members in one tx; duplicate adds are no-ops; the FIRST unknown
    /// prompt-id aborts the whole batch (atomicity over partial application).
    pub fn collection_add(&self, name: &str, prompt_ids: &[String]) -> Result<(usize, usize)> {
        let tx = self.conn().unchecked_transaction()?;
        for (idx, pid) in prompt_ids.iter().enumerate() {
            let exists: bool = tx
                .query_row(
                    "SELECT COUNT(*) > 0 FROM prompts WHERE id = ?1",
                    params![pid],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !exists {
                anyhow::bail!(format!("member_{idx}_not_found:{pid}"));
            }
        }
        let mut added = 0usize;
        for pid in prompt_ids {
            added += tx.execute(
                "INSERT OR IGNORE INTO collection_prompts (collection_name, prompt_id)
                 VALUES (?1, ?2)",
                params![name, pid],
            )?;
        }
        tx.commit()?;
        Ok((added, prompt_ids.len() - added))
    }

    pub fn collection_remove(&self, name: &str, prompt_ids: &[String]) -> Result<usize> {
        let mut removed = 0usize;
        for pid in prompt_ids {
            removed += self.conn().execute(
                "DELETE FROM collection_prompts WHERE collection_name = ?1 AND prompt_id = ?2",
                params![name, pid],
            )?;
        }
        Ok(removed)
    }

    pub fn collection_detail(&self, name: &str) -> Result<Option<CollectionDetail>> {
        let row = self
            .conn()
            .query_row(
                "SELECT name, description, created_at FROM collections WHERE name = ?1",
                params![name],
                |r| {
                    Ok(CollectionDetail {
                        name: r.get(0)?,
                        description: r.get(1)?,
                        created_at: r.get(2)?,
                        prompts: Vec::new(),
                    })
                },
            )
            .ok();
        let Some(mut detail) = row else {
            return Ok(None);
        };
        detail.prompts = {
            let mut stmt = self.conn().prepare(
                "SELECT prompt_id FROM collection_prompts WHERE collection_name = ?1 ORDER BY added_at ASC",
            )?;
            let rows = stmt.query_map(params![name], |r| r.get(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            out
        };
        Ok(Some(detail))
    }

    pub fn collections_list(&self) -> Result<Vec<CollectionDetail>> {
        let names: Vec<String> = {
            let mut stmt = self
                .conn()
                .prepare("SELECT name FROM collections ORDER BY name")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            out
        };
        let mut out = Vec::new();
        for n in names {
            if let Some(d) = self.collection_detail(&n)? {
                out.push(d);
            }
        }
        Ok(out)
    }
}
