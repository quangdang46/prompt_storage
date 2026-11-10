//! Contract tests for bead prompt_storage-p0-contract-harness-cleanup-batch-pst-x5q.
//!
//! Covers: interactive non-TTY → tty_required (never 'internal'),
//! doctor runs a real PRAGMA integrity_check, collections_list batch
//! correctness, and JSONL export alias batch lookup.

mod contract_support;
use contract_support::ContractEnv;

#[test]
fn interactive_non_tty_is_tty_required_not_internal() {
    let env = ContractEnv::new();
    env.seed_prompt("a", "A", "x");
    let (_o, err, code) = env.triple(&["interactive"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "tty_required");
    assert_ne!(v["error"], "internal");
}

#[test]
fn doctor_runs_real_integrity_check() {
    let env = ContractEnv::new();
    env.seed_prompt("a", "A", "x");
    let (out, _e, code) = env.triple(&["doctor", "--json"]);
    // warn-only exit (skill/editor checks) is fine; we assert the check ran.
    let _ = code;
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let checks = v["checks"].as_array().unwrap();
    let integrity = checks
        .iter()
        .find(|c| c["name"] == "database_integrity")
        .expect("database_integrity check present");
    assert_eq!(integrity["status"], "ok");
    assert_eq!(integrity["message"], "ok");
}

#[test]
fn collection_list_reports_member_counts() {
    let env = ContractEnv::new();
    env.seed_prompt("a", "A", "x");
    env.seed_prompt("b", "B", "y");
    env.seed_prompt("c", "C", "z");
    let (_o, _e, code) = env.triple(&["collection", "create", "team"]);
    assert_eq!(code, 0);
    let (_o, _e, code) = env.triple(&["collection", "add", "team", "a", "b"]);
    assert_eq!(code, 0);
    let (out, _e, code) = env.triple(&["collection", "list", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    let cols = v["collections"].as_array().unwrap();
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0]["name"], "team");
    assert_eq!(cols[0]["prompts"], 2);
}

#[test]
fn jsonl_export_includes_aliases_via_batch_lookup() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "content here");
    let (_o, _e, code) = env.triple(&["alias", "demo", "dl"]);
    assert_eq!(code, 0);
    let dir = env.home.path().join("export");
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("backup.jsonl");
    let path_str = out_path.to_string_lossy().into_owned();
    let (_o, _e, code) = env.triple(&["export", "--out", &path_str]);
    assert_eq!(code, 0);
    let text = std::fs::read_to_string(&out_path).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    // line 0 = _meta, line 1 = prompt with embedded aliases.
    assert!(lines.len() >= 2);
    let prompt_line: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let aliases = prompt_line["aliases"].as_array().unwrap();
    assert!(
        aliases.iter().any(|a| a == "dl"),
        "aliases missing in export: {prompt_line}"
    );
}
