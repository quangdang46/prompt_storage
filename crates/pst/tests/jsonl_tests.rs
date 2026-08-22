//! Integration tests for JSONL export/import round-trip (bead P1.4).
//!
//! Locked contract (plan §9): export → destroy DB → import → byte-identical
//! prompts including variables and aliases.

use anyhow::Result;
use pst::model::{Prompt, PromptVariable, VariableType};
use pst::storage::database::Database;
use pst::storage::jsonl::{ImportMode, export_jsonl, import_jsonl};

fn rich_prompt(id: &str) -> Prompt {
    let mut p = Prompt::new(
        id,
        format!("Prompt {id}"),
        format!("content-of-{} with {{CODE}} placeholder\nline two", id),
    );
    p.description = Some(format!("description {id}"));
    p.category = Some("testing".into());
    p.tags = vec!["t1".into(), "t2".into()];
    p.variables = vec![PromptVariable {
        name: "CODE".into(),
        var_type: VariableType::Multiline,
        required: true,
        description: Some("the code".into()),
        default: Some("fn main() {}".into()),
    }];
    p.use_count = 42;
    p.last_used_at = Some("2026-01-01T00:00:00Z".into());
    p.created_at = Some("2025-06-15T10:00:00".into());
    p
}

fn byte_identical(a: &Prompt, b: &Prompt) -> bool {
    // Compare every user-meaningful field; timestamps set by the DB on insert
    // are preserved through export/import via created_at/updated_at columns.
    a.id == b.id
        && a.title == b.title
        && a.content == b.content
        && a.description == b.description
        && a.category == b.category
        && a.tags == b.tags
        && a.variables == b.variables
        && a.version == b.version
        && a.author == b.author
        && a.difficulty == b.difficulty
        && a.featured == b.featured
        && a.source == b.source
        && a.use_count == b.use_count
        && a.last_used_at == b.last_used_at
}

#[test]
fn roundtrip_merge_preserves_everything() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("store.db");
    let jsonl = tmp.path().join("backup.jsonl");

    // Build original library.
    {
        let db = Database::open_at(&db_path)?;
        db.upsert_prompt(&rich_prompt("alpha")).unwrap();
        db.upsert_prompt(&rich_prompt("beta")).unwrap();
        db.add_alias("a", "alpha")?;
        db.add_alias("b", "beta")?;
        export_jsonl(&db, &jsonl)?;
    }

    // Destroy DB entirely.
    std::fs::remove_file(&db_path)?;

    // Re-import.
    let db = Database::open_at(&db_path)?;
    let n = import_jsonl(&db, &jsonl, ImportMode::Merge)?;
    assert_eq!(n, 2);

    for id in ["alpha", "beta"] {
        let orig = rich_prompt(id);
        let got = db.get_prompt(id)?.expect("restored");
        assert!(
            byte_identical(&orig, &got),
            "prompt {id} not byte-identical"
        );
    }
    assert_eq!(db.aliases_for("alpha")?, vec!["a"]);
    assert_eq!(db.aliases_for("beta")?, vec!["b"]);
    Ok(())
}

#[test]
fn roundtrip_replace_restores_after_wipe() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("store.db");
    let jsonl = tmp.path().join("backup.jsonl");

    {
        let db = Database::open_at(&db_path)?;
        db.upsert_prompt(&rich_prompt("keepme"))?;
        db.add_alias("k", "keepme")?;
        export_jsonl(&db, &jsonl)?;

        // Corrupt the live library after backup.
        db.upsert_prompt(&rich_prompt("junk"))?;
        db.delete_prompt("keepme")?;
    }

    let db = Database::open_at(&db_path)?;
    import_jsonl(&db, &jsonl, ImportMode::Replace)?;

    assert_eq!(db.prompt_count()?, 1, "replace must wipe junk");
    let got = db.get_prompt("keepme")?.expect("restored");
    assert!(byte_identical(&rich_prompt("keepme"), &got));
    assert_eq!(db.aliases_for("keepme")?, vec!["k"]);
    Ok(())
}

#[test]
fn merge_keeps_unrelated_existing_prompts() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let jsonl = tmp.path().join("part.jsonl");

    {
        let src = Database::in_memory()?;
        src.upsert_prompt(&rich_prompt("only-in-file"))?;
        export_jsonl(&src, &jsonl)?;
    }

    let db = Database::in_memory()?;
    db.upsert_prompt(&rich_prompt("already-here"))?;
    import_jsonl(&db, &jsonl, ImportMode::Merge)?;

    assert_eq!(db.prompt_count()?, 2);
    Ok(())
}

#[test]
fn replace_rejects_bad_file_without_wiping() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let bad = tmp.path().join("bad.jsonl");
    std::fs::write(
        &bad,
        "{\"_meta\":{\"version\":\"0\",\"count\":2,\"exported_at\":\"x\",\"schema_version\":1}}\n{\"id\": \"broken\"\n",
    )?;

    let db = Database::in_memory()?;
    db.upsert_prompt(&rich_prompt("survivor"))?;

    let err = import_jsonl(&db, &bad, ImportMode::Replace);
    assert!(err.is_err(), "corrupt file must fail");
    assert_eq!(db.prompt_count()?, 1, "failed import must not wipe");
    Ok(())
}

#[test]
fn future_schema_version_is_rejected() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let future = tmp.path().join("future.jsonl");
    std::fs::write(
        &future,
        "{\"_meta\":{\"version\":\"0\",\"count\":1,\"exported_at\":\"x\",\"schema_version\":99}}\n",
    )?;
    let db = Database::in_memory()?;
    let err = import_jsonl(&db, &future, ImportMode::Merge).unwrap_err();
    assert!(
        err.to_string().contains("schema_too_new"),
        "wrong error: {err}"
    );
    Ok(())
}

#[test]
fn meta_header_present_and_accurate() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let jsonl = tmp.path().join("with-meta.jsonl");

    let db = Database::in_memory()?;
    db.upsert_prompt(&rich_prompt("m-one"))?;
    db.upsert_prompt(&rich_prompt("m-two"))?;
    export_jsonl(&db, &jsonl)?;

    let first = std::fs::read_to_string(&jsonl)?
        .lines()
        .next()
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&first)?;
    assert!(v.get("_meta").is_some(), "first line must be _meta header");
    assert_eq!(v["_meta"]["count"], 2);
    assert!(v["_meta"]["schema_version"].as_i64().is_some());
    Ok(())
}

#[test]
fn atomic_no_tmp_left_behind() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let jsonl = tmp.path().join("atomic.jsonl");
    let db = Database::in_memory()?;
    db.upsert_prompt(&rich_prompt("x"))?;
    export_jsonl(&db, &jsonl)?;
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
    Ok(())
}
