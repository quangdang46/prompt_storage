//! Contract regression tests for export-ids-ignored-uxh:
//! --ids/--all must select the prompt set for EVERY export format.

mod contract_support;

use contract_support::ContractEnv;

fn jsonl_ids(path: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .collect()
}

#[test]
fn export_ids_filters_jsonl_file() {
    let env = ContractEnv::new();
    env.seed_prompt("only-this", "OT", "x");
    env.seed_prompt("not-that", "NT", "y");

    let out = env.home.path().join("sub").join("e.jsonl");
    let out_arg = format!("--out={}", out.display());
    let (stdout, stderr, code) = env.triple(&[
        "export",
        "--ids",
        "only-this",
        "--format",
        "jsonl",
        &out_arg,
    ]);
    assert_eq!(code, 0, "{stdout} / {stderr}");
    assert_eq!(jsonl_ids(&out), vec!["only-this".to_string()]);
}

#[test]
fn export_all_and_default_export_everything_jsonl() {
    let env = ContractEnv::new();
    env.seed_prompt("p1", "P1", "x");
    env.seed_prompt("p2", "P2", "y");

    let out = env.home.path().join("all.jsonl");
    let out_arg = format!("--out={}", out.display());
    let (_, _, code) = env.triple(&["export", "--all", &out_arg]);
    assert_eq!(code, 0);
    let mut ids = jsonl_ids(&out);
    ids.sort();
    assert_eq!(ids, vec!["p1".to_string(), "p2".to_string()]);

    // Default (no --ids/--all) also exports everything.
    let out2 = env.home.path().join("def.jsonl");
    let out2_arg = format!("--out={}", out2.display());
    let (_, _, code) = env.triple(&["export", &out2_arg]);
    assert_eq!(code, 0);
    assert_eq!(jsonl_ids(&out2).len(), 2);
}

#[test]
fn export_ids_filters_markdown_dir() {
    let env = ContractEnv::new();
    env.seed_prompt("keep-md", "KM", "x");
    env.seed_prompt("drop-md", "DM", "y");

    let dir = env.home.path().join("mdout");
    let dir_arg = format!("--out={}", dir.display());
    let (_, stderr, code) = env.triple(&[
        "export", "--ids", "keep-md", "--format", "markdown", &dir_arg,
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(dir.join("keep-md.md").exists());
    assert!(!dir.join("drop-md.md").exists());
}

#[test]
fn export_stdout_works_for_jsonl_and_honors_ids() {
    let env = ContractEnv::new();
    env.seed_prompt("s1", "S1", "x");
    env.seed_prompt("s2", "S2", "y");

    let (stdout, _, code) = env.triple(&["export", "--ids", "s1", "--stdout"]);
    assert_eq!(code, 0);
    let ids = jsonl_ids_from_str(&stdout);
    assert_eq!(ids, vec!["s1".to_string()]);
}

fn jsonl_ids_from_str(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .collect()
}

#[test]
fn export_invalid_format_is_usage_error() {
    let env = ContractEnv::new();
    let (_, stderr, code) = env.triple(&["export", "--format", "xml", "--out", "/tmp/x"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(v["error"], "invalid_format");
}
