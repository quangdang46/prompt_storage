//! Unit tests for the database layer (bead P1.3).
//!
//! All tests run on in-memory databases — zero disk contact (plan §4 rule 5).

use pst::model::{Prompt, PromptVariable, VariableType};
use pst::storage::database::{Database, escape_fts_query};

fn sample_prompt(id: &str) -> Prompt {
    let mut p = Prompt::new(
        id,
        "Code Review Assistant",
        "Review this {{LANGUAGE}} code for bugs:\n\n{{CODE}}",
    );
    p.description = Some("Comprehensive review".into());
    p.category = Some("debugging".into());
    p.tags = vec!["review".into(), "quality".into()];
    p.variables = vec![PromptVariable {
        name: "CODE".into(),
        var_type: VariableType::Multiline,
        required: true,
        description: None,
        default: None,
    }];
    p
}

#[test]
fn upsert_get_roundtrip_preserves_children() {
    let db = Database::in_memory().unwrap();
    let p = sample_prompt("code-review");
    db.upsert_prompt(&p).unwrap();

    let got = db.get_prompt("code-review").unwrap().expect("exists");
    assert_eq!(got.title, "Code Review Assistant");
    assert_eq!(got.category.as_deref(), Some("debugging"));
    assert_eq!(got.tags.len(), 2);
    assert_eq!(got.variables.len(), 1);
    assert_eq!(got.variables[0].name, "CODE");
    assert_eq!(got.variables[0].var_type, VariableType::Multiline);
    assert!(got.variables[0].required);
}

#[test]
fn upsert_is_update_not_duplicate() {
    let db = Database::in_memory().unwrap();
    let mut p = sample_prompt("demo");
    db.upsert_prompt(&p).unwrap();
    p.title = "Updated Title".into();
    p.tags = vec!["newtag".into()];
    db.upsert_prompt(&p).unwrap();

    assert_eq!(db.prompt_count().unwrap(), 1);
    let got = db.get_prompt("demo").unwrap().unwrap();
    assert_eq!(got.title, "Updated Title");
    assert_eq!(got.tags, vec!["newtag"]);
}

#[test]
fn upsert_preserves_use_count_across_updates() {
    let db = Database::in_memory().unwrap();
    let p = sample_prompt("demo");
    db.upsert_prompt(&p).unwrap();
    db.bump_usage("demo").unwrap();
    db.bump_usage("demo").unwrap();
    db.upsert_prompt(&p).unwrap(); // re-add same content
    let got = db.get_prompt("demo").unwrap().unwrap();
    assert_eq!(got.use_count, 2, "upsert must not reset use_count");
}

#[test]
fn fts_stays_in_sync_after_every_mutation() {
    let db = Database::in_memory().unwrap();
    db.upsert_prompt(&sample_prompt("review-a")).unwrap();

    // FTS row exists and is searchable.
    let hits = db.search("review", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Update content — FTS must follow.
    let mut p = sample_prompt("review-a");
    p.content = "Now about DATABASE tuning and indexes".into();
    db.upsert_prompt(&p).unwrap();
    let hits = db.search("database", 10).unwrap();
    assert_eq!(hits.len(), 1);
    let hits = db.search("reviewable-content-gone", 10).unwrap();
    assert!(hits.is_empty());

    // Delete — FTS row must go too.
    db.delete_prompt("review-a").unwrap();
    let fts_rows: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM prompts_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fts_rows, 0, "FTS row must cascade away with its prompt");
}

#[test]
fn search_ranks_and_negates_bm25_scores() {
    let db = Database::in_memory().unwrap();
    let mut strong = sample_prompt("security-review");
    strong.content = "security audit checklist: authentication, authorization, secrets".into();
    strong.tags = vec!["security".into()];
    db.upsert_prompt(&strong).unwrap();

    let mut other = sample_prompt("garden-tips");
    other.title = "Garden Tips".into();
    other.content = "how to water plants".into();
    db.upsert_prompt(&other).unwrap();

    let hits = db.search("security", 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].0.id, "security-review");
    // Scores are negated → positive now, descending order.
    for w in hits.windows(2) {
        assert!(w[0].1 >= w[1].1, "results must be score-descending");
    }
}

#[test]
fn search_escapes_special_characters_without_panicking() {
    let db = Database::in_memory().unwrap();
    db.upsert_prompt(&sample_prompt("plain")).unwrap();
    // These inputs historically crash unescaped FTS5 MATCH parsers.
    for q in [
        "\"quoted phrase\"",
        "NOT/AND/OR*",
        "(parens) AND [brackets]",
        "a-b_c^d",
        "-leading-operator",
    ] {
        let _ = db.search(q, 5).unwrap(); // must not panic or err
    }
    let _ = escape_fts_query("multi \"word\" query"); // direct check
}

#[test]
fn list_filters_work_in_combination() {
    let db = Database::in_memory().unwrap();
    let mut a = sample_prompt("rust-review");
    a.category = Some("coding".into());
    a.featured = true;
    let mut b = sample_prompt("js-review");
    b.category = Some("web".into());
    db.upsert_prompt(&a).unwrap();
    db.upsert_prompt(&b).unwrap();

    let all = db.list_prompts_filtered(None, None, false).unwrap();
    assert_eq!(all.len(), 2);

    let coding = db
        .list_prompts_filtered(Some("coding"), None, false)
        .unwrap();
    assert_eq!(coding.len(), 1);
    assert_eq!(coding[0].id, "rust-review");

    let featured = db.list_prompts_filtered(None, None, true).unwrap();
    assert_eq!(featured.len(), 1);

    // Tag filter via join table
    let tagged = db
        .list_prompts_filtered(None, Some("quality"), false)
        .unwrap();
    assert_eq!(tagged.len(), 2); // both have "review"+"quality" tags from sample

    let combo = db
        .list_prompts_filtered(Some("coding"), Some("quality"), false)
        .unwrap();
    assert_eq!(combo.len(), 1);
    assert_eq!(combo[0].id, "rust-review");
}

#[test]
fn taxonomy_counts_are_sorted_correctly() {
    let db = Database::in_memory().unwrap();
    let mut a = sample_prompt("p-one");
    a.category = Some("zeta".into());
    a.tags = vec!["shared".into(), "alpha-only".into()];
    let mut b = sample_prompt("p-two");
    b.category = Some("alpha".into());
    b.tags = vec!["shared".into()];
    let mut c = sample_prompt("p-three");
    c.category = Some("alpha".into());
    c.tags = vec!["shared".into()];
    db.upsert_prompt(&a).unwrap();
    db.upsert_prompt(&b).unwrap();
    db.upsert_prompt(&c).unwrap();

    let cats = db.category_counts().unwrap();
    assert_eq!(
        cats,
        vec![("alpha".to_string(), 2), ("zeta".to_string(), 1)]
    );

    let tags = db.tag_counts().unwrap();
    assert_eq!(tags[0], ("shared".to_string(), 3)); // count desc first
}

#[test]
fn alias_collision_invariants_enforced() {
    let db = Database::in_memory().unwrap();
    db.upsert_prompt(&sample_prompt("foo")).unwrap();

    // alias == existing canonical id → id_conflict
    let e = db.add_alias("foo", "foo").unwrap_err();
    assert_eq!(e.to_string(), "id_conflict");

    // alias == canonical id case-insensitively → still id_conflict
    let e = db.add_alias("FOO", "foo").unwrap_err();
    assert_eq!(e.to_string(), "id_conflict");

    // valid alias works
    db.add_alias("f", "foo").unwrap();
    assert_eq!(db.lookup_alias("f").unwrap().as_deref(), Some("foo"));

    // same alias different prompt → alias_conflict
    db.upsert_prompt(&sample_prompt("bar")).unwrap();
    let e = db.add_alias("f", "bar").unwrap_err();
    assert_eq!(e.to_string(), "alias_conflict");

    // case-insensitive duplicate alias → alias_conflict
    let e = db.add_alias("F", "bar").unwrap_err();
    assert_eq!(e.to_string(), "alias_conflict");

    // idempotent re-add of identical mapping is fine
    db.add_alias("f", "foo").unwrap();

    // alias pointing at nonexistent prompt → not_found
    let e = db.add_alias("zz", "ghost").unwrap_err();
    assert_eq!(e.to_string(), "not_found");

    // remove works
    assert!(db.remove_alias("f").unwrap());
    assert!(!db.remove_alias("f").unwrap()); // second remove: already gone
}

#[test]
fn delete_cascades_all_children() {
    let db = Database::in_memory().unwrap();
    db.upsert_prompt(&sample_prompt("doomed")).unwrap();
    db.add_alias("d", "doomed").unwrap();

    assert!(db.delete_prompt("doomed").unwrap());
    assert!(!db.delete_prompt("doomed").unwrap()); // already gone

    assert!(db.get_prompt("doomed").unwrap().is_none());
    assert!(db.aliases_for("doomed").unwrap().is_empty());
    let vars: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM prompt_variables", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vars, 0);
}

#[test]
fn meta_roundtrip() {
    let db = Database::in_memory().unwrap();
    assert_eq!(db.get_meta("data_version").unwrap(), None);
    db.set_meta("data_version", "2026-08-22T00:00:00Z").unwrap();
    assert_eq!(
        db.get_meta("data_version").unwrap().as_deref(),
        Some("2026-08-22T00:00:00Z")
    );
    db.set_meta("data_version", "later").unwrap();
    assert_eq!(
        db.get_meta("data_version").unwrap().as_deref(),
        Some("later")
    );
}

#[test]
fn summaries_exclude_children_but_include_tags() {
    let db = Database::in_memory().unwrap();
    db.upsert_prompt(&sample_prompt("sum-test")).unwrap();
    let sums = db.list_summaries().unwrap();
    assert_eq!(sums.len(), 1);
    assert_eq!(sums[0].tags.len(), 2); // tags present in summary
}
