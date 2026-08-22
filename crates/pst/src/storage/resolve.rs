//! Friendly-ID resolution engine (plan §5) — the heart of `pst <query>`.
//!
//! Decision order:
//! 1. EXACT  — canonical id match
//! 2. ALIAS  — NOCASE alias match
//! 3. PREFIX — unique prefix over ids ∪ aliases; >1 → AMBIGUOUS
//! 4. FUZZY  — FTS BM25 top-k=8; top1 must beat top2 by ≥40% dominance
//!    ratio, else AMBIGUOUS; none → NOT_FOUND
//!
//! HARD RULE: never pick when unclear. Prefix/fuzzy hits bump use_count.

use anyhow::Result;
use serde::Serialize;

use super::database::Database;

/// Pinned constants (bead polish R1) so tests can assert exact thresholds.
pub const FUZZY_TOPK: usize = 8;
pub const FUZZY_DOMINANCE: f64 = 1.40;
pub const FUZZY_MIN_SCORE: f64 = 0.5;
pub const PREFIX_CANDIDATE_CAP: usize = 10;

/// Where a resolution came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolveSource {
    Exact,
    Alias,
    Prefix,
    Fuzzy,
}

/// One candidate row for ambiguous payloads.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub score: Option<f64>,
    pub source: &'static str,
}

/// Resolution outcome.
#[derive(Debug)]
pub enum ResolveOutcome {
    Hit {
        id: String,
        title: String,
        source: ResolveSource,
    },
    Ambiguous {
        query: String,
        candidates: Vec<Candidate>,
    },
    NotFound {
        query: String,
    },
}

impl ResolveOutcome {
    pub fn is_hit(&self) -> bool {
        matches!(self, ResolveOutcome::Hit { .. })
    }
}

/// Resolve `query` against the library. Bumps use_count on prefix/fuzzy hits.
pub fn resolve(db: &Database, query: &str) -> Result<ResolveOutcome> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(ResolveOutcome::NotFound { query: q.into() });
    }

    // 1. EXACT
    if let Some(p) = db.get_prompt(q)? {
        return Ok(ResolveOutcome::Hit {
            id: p.id,
            title: p.title,
            source: ResolveSource::Exact,
        });
    }

    // 2. ALIAS (NOCASE handled by collation in schema)
    if let Some(p) = db
        .lookup_alias(q)?
        .and_then(|id| db.get_prompt(&id).transpose())
        .transpose()?
    {
        return Ok(ResolveOutcome::Hit {
            id: p.id,
            title: p.title,
            source: ResolveSource::Alias,
        });
    }

    // 3. PREFIX over ids ∪ aliases
    let prefix_hits = collect_prefix_candidates(db, q)?;
    match prefix_hits.len() {
        1 => {
            let c = &prefix_hits[0];
            if let Some(p) = db.get_prompt(&c.id)? {
                db.bump_usage(&p.id)?;
                return Ok(ResolveOutcome::Hit {
                    id: p.id,
                    title: p.title,
                    source: ResolveSource::Prefix,
                });
            }
        }
        n if n > 1 => {
            return Ok(ResolveOutcome::Ambiguous {
                query: q.into(),
                candidates: prefix_hits,
            });
        }
        _ => {}
    }

    // 4. FUZZY via FTS BM25
    let hits = db.search(q, FUZZY_TOPK)?;
    if hits.is_empty() {
        return Ok(ResolveOutcome::NotFound { query: q.into() });
    }

    let top_score = hits[0].1;
    let second_score = hits.get(1).map(|(_, s)| *s);

    // Dominance test: top must beat runner-up by FUZZY_DOMINANCE ratio.
    // BM25 magnitudes scale with corpus size, so a fixed floor is useless —
    // the RATIO is the signal (pinned constant, bead polish R1).
    let dominant = match second_score {
        None => true,
        Some(second) => second <= 0.0 || top_score >= second * FUZZY_DOMINANCE,
    };

    if !dominant {
        return candidates_from_hits(db, q, &hits);
    }

    let (p, _) = &hits[0];
    db.bump_usage(&p.id)?;
    Ok(ResolveOutcome::Hit {
        id: p.id.clone(),
        title: p.title.clone(),
        source: ResolveSource::Fuzzy,
    })
}

fn candidates_from_hits(
    db: &Database,
    query: &str,
    hits: &[(crate::model::Prompt, f64)],
) -> Result<ResolveOutcome> {
    let mut candidates = Vec::new();
    for (p, score) in hits {
        candidates.push(Candidate {
            id: p.id.clone(),
            title: p.title.clone(),
            score: Some(*score),
            source: "fuzzy",
        });
    }
    let _ = db;
    Ok(ResolveOutcome::Ambiguous {
        query: query.into(),
        candidates,
    })
}

/// Collect prefix matches over canonical ids and aliases.
/// Aliases resolve to their owning prompt. Sorted use_count DESC then alpha.
fn collect_prefix_candidates(db: &Database, prefix: &str) -> Result<Vec<Candidate>> {
    let pattern = format!("{}%", prefix.replace(['%', '_'], ""));
    let mut stmt = db.conn().prepare(
        r#"
        SELECT DISTINCT p.id, p.title, p.use_count FROM prompts p
        WHERE p.id LIKE ?1
        UNION ALL
        SELECT DISTINCT p.id, p.title, p.use_count FROM aliases a
        JOIN prompts p ON p.id = a.prompt_id
        WHERE a.alias LIKE ?1
        "#,
    )?;
    let rows = stmt.query_map(rusqlite::params![pattern], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    // Dedupe (a prompt can be reached via both its id and an alias).
    let mut seen = std::collections::BTreeSet::new();
    let mut out: Vec<(String, String, i64)> = Vec::new();
    for r in rows {
        let triple = r?;
        if seen.insert(triple.0.clone()) {
            out.push(triple);
        }
    }

    // Sort: use_count DESC, then id alphabetically.
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out.truncate(PREFIX_CANDIDATE_CAP);

    Ok(out
        .into_iter()
        .map(|(id, title, _)| Candidate {
            id,
            title,
            score: None,
            source: "prefix",
        })
        .collect())
}

#[cfg(test)]
mod decision_tests;
