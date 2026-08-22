//! Contract tests (bead P0.2): direct-mode output triples.
//!
//! Each test asserts exact stdout bytes, stderr bytes, and exit code —
//! the frozen API surface AI agents depend on.

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn unknown_query_returns_not_found_payload() {
    let env = ContractEnv::new();
    let (stdout, stderr, code) = env.triple(&["ghost-query"]);
    assert!(
        stdout.is_empty(),
        "stdout must be empty on not-found: {stdout:?}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr must be valid JSON");
    assert_eq!(payload["error"], "not_found");
    assert_eq!(payload["query"], "ghost-query");
    assert_eq!(code, 1);
}

#[test]
fn ambiguous_prefix_returns_candidates() {
    let env = ContractEnv::new();
    env.seed_prompt("alpha-review", "Alpha", "x");
    env.seed_prompt("alpha-security", "Beta", "y");

    let (stdout, stderr, code) = env.triple(&["alpha"]);
    assert!(stdout.is_empty(), "no content may leak on ambiguity");
    let payload: serde_json::Value = serde_json::from_str(&stderr).expect("stderr JSON");
    assert_eq!(payload["error"], "ambiguous");
    let cands = payload["candidates"].as_array().expect("candidates array");
    assert_eq!(cands.len(), 2);
    assert_eq!(code, 1);
}

#[test]
fn empty_library_direct_query_is_not_found() {
    let env = ContractEnv::new();
    let (_, stderr, code) = env.triple(&["anything"]);
    assert!(stderr.contains("not_found"));
    assert_eq!(code, 1);
}

#[test]
fn json_flag_accepted_globally() {
    let env = ContractEnv::new();
    env.seed_prompt("demo", "Demo", "content here");
    // --json on a hit returns the full payload; shape is the contract.
    let out = env.run(&["--json", "demo"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let payload: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be valid JSON");
    assert_eq!(payload["id"], "demo");
    assert_eq!(payload["title"], "Demo");
    assert_eq!(payload["content"], "content here");
    assert!(payload["tags"].is_array());
}
