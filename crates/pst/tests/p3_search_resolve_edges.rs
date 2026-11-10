//! Contract tests for bead prompt_storage-p0-contract-harness-search-resolve-edge-batch-ats.
//!
//! Covers: degenerate FTS queries, LIKE-escaped prefix resolution over
//! aliases containing '_'/'%', alias shape validation, and DB-error
//! propagation (no .ok()/unwrap_or(false) conflation).

mod contract_support;
use contract_support::ContractEnv;

#[test]
fn alias_with_underscore_is_prefix_resolvable() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "unrelated zzz content");
    let (_o, _e, code) = env.triple(&["alias", "demo", "my_alias"]);
    assert_eq!(code, 0);
    // Prefix "my_al" must reach alias "my_alias" literally.
    let (out, _e, code) = env.triple(&["my_al"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "unrelated zzz content");
}

#[test]
fn underscore_in_query_does_not_overmatch_via_like_wildcard() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "zzz unrelated");
    let (_o, _e, code) = env.triple(&["alias", "demo", "x_y"]);
    assert_eq!(code, 0);
    // "xay" must NOT prefix-match alias "x_y" — '_' is literal now.
    // Fuzzy also misses (content has no 'xay'), so expect not_found.
    let (_o, err, code) = env.triple(&["xay"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "not_found");
}

#[test]
fn degenerate_queries_never_surface_internal_error() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "hello world");
    for q in ["!!!", "(((", "— — —"] {
        let (_o, _err, code) = env.triple(&["search", q]);
        assert_eq!(code, 0, "search {q:?} must not error");
        let (_o, _err, code) = env.triple(&["suggest", q]);
        assert_eq!(code, 0, "suggest {q:?} must not error");
        let (_o, err2, code2) = env.triple(&[q]);
        // Direct mode: not_found is correct; 'internal' is the bug.
        assert_ne!(code2, -1);
        let v: serde_json::Value = serde_json::from_str(err2.trim()).unwrap();
        assert_ne!(v["error"], "internal", "direct {q:?}: {err2}");
    }
}

#[test]
fn empty_and_whitespace_aliases_rejected() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "content");
    let (_o, err, code) = env.triple(&["alias", "demo", ""]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid_alias"), "stderr: {err}");
    let (_o, err, code) = env.triple(&["alias", "demo", "   "]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid_alias"), "stderr: {err}");
}

#[test]
fn percent_sign_in_prefix_query_is_literal() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "zzz");
    env.seed_prompt("other", "Other", "yyy");
    let (_o, _e, _c) = env.triple(&["alias", "demo", "pct%alias"]);
    // Query "pct" must reach "pct%alias" literally, not treat % as wildcard
    // matching every "pct..." alias — with only one alias it resolves.
    let (out, _e, code) = env.triple(&["pct"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "zzz");
}
