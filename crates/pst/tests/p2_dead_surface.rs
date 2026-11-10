//! Contract tests for bead prompt_storage-p0-contract-harness-dead-surface-batch-pbl.
//!
//! Covers: `pst suggest` (previously referenced by SKILL.md but missing),
//! config output.json as a real default, and suggest.default_limit wiring.

mod contract_support;
use contract_support::ContractEnv;

#[test]
fn suggest_returns_ranked_results_with_reasons() {
    let env = ContractEnv::new();
    env.seed_prompt(
        "code-review",
        "Code review",
        "Review {{LANG}} code for bugs",
    );
    env.seed_prompt(
        "git-commit",
        "Git commit",
        "Write a conventional commit message",
    );

    let (out, _err, code) = env.triple(&["suggest", "review code"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    let suggestions = v["suggestions"].as_array().unwrap();
    assert!(!suggestions.is_empty());
    let first = &suggestions[0];
    assert_eq!(first["id"], "code-review");
    assert!(first["reason"].as_str().unwrap().contains("title"));
    assert!(first["score"].is_number());
}

#[test]
fn suggest_limit_flag_and_config_default() {
    let env = ContractEnv::new();
    for i in 0..3 {
        env.seed_prompt(
            &format!("review-{i}"),
            &format!("Review prompt {i}"),
            "review the code carefully",
        );
    }
    // --limit 1 caps results.
    let (out, _e, code) = env.triple(&["suggest", "review", "-l", "1"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"], 1);

    // Config suggest.default_limit = 2 caps results without a flag.
    let (_o, _e, code) = env.triple(&["config", "set", "suggest.default_limit", "2"]);
    assert_eq!(code, 0);
    let (out, _e, code) = env.triple(&["suggest", "review"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"], 2);
}

#[test]
fn output_json_config_makes_list_emit_json_without_flag() {
    let env = ContractEnv::new();
    env.seed_prompt("p1", "One", "content");
    let (_o, _e, code) = env.triple(&["config", "set", "output.json", "true"]);
    assert_eq!(code, 0);
    let (out, _e, code) = env.triple(&["list"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v["prompts"].is_array());
}

#[test]
fn select_variable_options_validated_and_persisted() {
    let env = ContractEnv::new();
    let mut cmd = env.command(&[
        "new",
        "selp",
        "--var",
        "MODE=select=fast|safe|thorough",
        "--from",
        "-",
    ]);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"ship in {{MODE}} mode")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 0);

    // Default applies.
    let (out, _e, code) = env.triple(&["render", "selp"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "ship in fast mode");

    // Valid option accepted.
    let (out, _e, code) = env.triple(&["render", "selp", "--MODE=safe"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "ship in safe mode");

    // Invalid option rejected.
    let (_o, err, code) = env.triple(&["render", "selp", "--MODE=slow"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid_option"), "stderr: {err}");

    // Options round-trip through show --json.
    let (out, _e, code) = env.triple(&["show", "selp", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["variables"][0]["options"],
        serde_json::json!(["fast", "safe", "thorough"])
    );
}

#[test]
fn suggest_empty_query_is_usage_error() {
    let env = ContractEnv::new();
    let (_o, err, code) = env.triple(&["suggest", "   "]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "empty_query");
}
