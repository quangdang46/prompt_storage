//! Exhaustive decision-matrix tests for the resolution engine (bead P2.1).

use crate::model::Prompt;
use crate::storage::database::Database;
use crate::storage::resolve::{ResolveOutcome, ResolveSource, resolve};
use anyhow::Result;

fn seed(db: &Database) -> Result<()> {
    let mut a = Prompt::new("code-review", "Code Review Assistant", "review content");
    a.tags = vec!["review".into()];
    let mut b = Prompt::new("security-review", "Security Review", "security content");
    b.tags = vec!["security".into()];
    db.upsert_prompt(&a)?;
    db.upsert_prompt(&b)?;
    Ok(())
}

#[test]
fn exact_id_hits_without_usage_bump() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    match resolve(&db, "code-review")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "code-review");
            assert_eq!(source, ResolveSource::Exact);
        }
        _ => panic!("expected exact hit"),
    }
    let p = db.get_prompt("code-review")?.unwrap();
    assert_eq!(p.use_count, 0, "exact hits must not bump use_count");
    Ok(())
}

#[test]
fn alias_hits_nocase() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    db.add_alias("cr", "code-review")?;
    for q in ["cr", "CR"] {
        match resolve(&db, q)? {
            ResolveOutcome::Hit { id, source, .. } => {
                assert_eq!(id, "code-review");
                assert_eq!(source, ResolveSource::Alias);
            }
            _ => panic!("expected alias hit for {q}"),
        }
    }
    Ok(())
}

#[test]
fn unique_prefix_hits_and_bumps_usage() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    // "cod" only matches code-review
    match resolve(&db, "cod")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "code-review");
            assert_eq!(source, ResolveSource::Prefix);
        }
        _ => panic!("expected prefix hit"),
    }
    let p = db.get_prompt("code-review")?.unwrap();
    assert_eq!(p.use_count, 1, "prefix hits bump use_count");
    Ok(())
}

#[test]
fn multi_prefix_is_ambiguous_with_sorted_candidates() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    // "code-review" vs "security-review" share no prefix; use "review" via alias instead
    db.add_alias("review-a", "code-review")?;
    db.add_alias("review-b", "security-review")?;

    match resolve(&db, "review-")? {
        ResolveOutcome::Ambiguous { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
            // both zero usage → alphabetical tie-break
            assert_eq!(candidates[0].id, "code-review");
            assert_eq!(candidates[1].id, "security-review");
        }
        _ => panic!("expected ambiguous"),
    }
    Ok(())
}

#[test]
fn prefix_tiebreak_prefers_frequently_used() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    db.add_alias("rev", "code-review")?;
    db.add_alias("rev2", "security-review")?;

    // Bump security-review so it wins the "rev" prefix race.
    db.bump_usage("security-review")?;
    db.bump_usage("security-review")?;
    db.bump_usage("security-review")?;

    // "rev" is an EXACT alias → hits code-review directly (step 2 wins).
    match resolve(&db, "rev")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "code-review");
            assert_eq!(source, ResolveSource::Alias);
        }
        _ => panic!("exact alias must hit"),
    }

    // "re" is a PREFIX matching both aliases → ambiguous with the
    // frequently-used prompt listed first.
    match resolve(&db, "re")? {
        ResolveOutcome::Ambiguous { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
            assert_eq!(candidates[0].id, "security-review", "usage DESC ordering");
        }
        _ => panic!("multi-match must stay ambiguous"),
    }

    // "rev2" is itself an exact alias → step 2 wins (Alias, not Prefix).
    match resolve(&db, "rev2")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "security-review");
            assert_eq!(source, ResolveSource::Alias);
        }
        _ => panic!("exact alias should hit"),
    }

    // A true unique prefix over ids: "se" only matches security-review.
    match resolve(&db, "se")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "security-review");
            assert_eq!(source, ResolveSource::Prefix);
        }
        _ => panic!("unique prefix should hit"),
    }
    Ok(())
}

#[test]
fn decisive_fuzzy_hit_warns_but_resolves() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    // Query matching one prompt strongly through FTS content.
    match resolve(&db, "security content audit")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "security-review");
            assert_eq!(source, ResolveSource::Fuzzy);
        }
        other => panic!("expected fuzzy hit, got {other:?}"),
    }
    let p = db.get_prompt("security-review")?.unwrap();
    assert!(p.use_count >= 1, "fuzzy hits bump usage");
    Ok(())
}

#[test]
fn weak_fuzzy_signal_stays_ambiguous() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    // Both prompts contain "review" in title with similar weights → not decisive.
    match resolve(&db, "review")? {
        ResolveOutcome::Ambiguous { candidates, .. } => {
            assert!(candidates.len() >= 2, "ties must list all candidates");
            for c in &candidates {
                assert!(c.score.is_some(), "fuzzy candidates carry scores");
            }
        }
        ResolveOutcome::Hit { .. } => panic!("weak signal must NOT auto-resolve"),
        _ => {}
    }
    Ok(())
}

#[test]
fn nothing_matches_returns_not_found() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    match resolve(&db, "zzz-nonexistent")? {
        ResolveOutcome::NotFound { query } => assert_eq!(query, "zzz-nonexistent"),
        _ => panic!("expected not_found"),
    }
    // Empty query also NotFound
    assert!(matches!(resolve(&db, "")?, ResolveOutcome::NotFound { .. }));
    assert!(matches!(
        resolve(&db, "   ")?,
        ResolveOutcome::NotFound { .. }
    ));
    Ok(())
}

#[test]
fn alias_prefix_resolves_to_owner() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    db.add_alias("myalias", "code-review")?;
    match resolve(&db, "myal")? {
        ResolveOutcome::Hit { id, source, .. } => {
            assert_eq!(id, "code-review");
            assert_eq!(source, ResolveSource::Prefix);
        }
        _ => panic!("alias prefix should reach owner"),
    }
    Ok(())
}

#[test]
fn case_insensitive_alias_exact_match() -> Result<()> {
    let db = Database::in_memory()?;
    seed(&db)?;
    db.add_alias("MyAlias", "code-review")?;
    match resolve(&db, "myalias")? {
        ResolveOutcome::Hit { source, .. } => assert_eq!(source, ResolveSource::Alias),
        _ => panic!("NOCASE alias lookup failed"),
    }
    Ok(())
}
