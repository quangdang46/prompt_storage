//! Contract tests for core commands (bead P2.3): new, show, rm, alias, unalias.

mod contract_support;

use contract_support::ContractEnv;
use std::io::Write as _;

/// Spawn `pst` with piped stdin containing `content`; capture full output.
fn run_with_stdin(env: &ContractEnv, args: &[&str], content: &str) -> (String, String, i32) {
    let mut child = env
        .command(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn pst");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(content.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn new_from_stdin_creates_prompt_then_direct_get_works() {
    let env = ContractEnv::new();
    let (stdout, stderr, code) = run_with_stdin(
        &env,
        &["new", "demo", "--title", "Demo Prompt", "-f", "-"],
        "line one\nline two\n",
    );
    assert_eq!(code, 0, "new must succeed: {stdout} / {stderr}");

    // Direct get returns byte-exact content.
    let (stdout, stderr, code) = env.triple(&["demo"]);
    assert_eq!(stdout, "line one\nline two\n");
    assert!(stderr.is_empty());
    assert_eq!(code, 0);
}

#[test]
fn new_duplicate_fails_without_force() {
    let env = ContractEnv::new();
    env.seed_prompt("dup", "Dup", "x");
    let (stdout, stderr, code) = run_with_stdin(&env, &["new", "dup", "-f", "-"], "y");
    assert_eq!(code, 1);
    let payload: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr JSON: {e}; raw={stderr:?} stdout={stdout:?}"));
    assert_eq!(payload["error"], "already_exists");
}

#[test]
fn new_rejects_invalid_id() {
    let env = ContractEnv::new();
    let (stdout, stderr, code) = run_with_stdin(&env, &["new", "Bad_ID", "-f", "-"], "x");
    assert_eq!(code, 1);
    let payload: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr JSON: {e}; raw={stderr:?} stdout={stdout:?}"));
    assert_eq!(payload["error"], "invalid_id");
}

#[test]
fn new_empty_content_rejected() {
    let env = ContractEnv::new();
    let (_, stderr, code) = run_with_stdin(&env, &["new", "empty-one", "-f", "-"], "  \n  ");
    assert_eq!(code, 1);
    let payload: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr must be the error payload");
    assert_eq!(payload["error"], "empty_content");
}

#[test]
fn show_outputs_metadata_not_raw_content() {
    let env = ContractEnv::new();
    env.seed_prompt("meta-demo", "Meta Demo", "SECRET-CONTENT-MARKER");

    let (stdout, _, code) = env.triple(&["show", "meta-demo"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Meta Demo"), "title present: {stdout}");
    assert!(stdout.contains("meta-demo"), "id present");
    assert!(
        !stdout.contains("SECRET-CONTENT-MARKER"),
        "show must never print raw content"
    );
}

#[test]
fn rm_deletes_and_subsequent_get_404s() {
    let env = ContractEnv::new();
    env.seed_prompt("doomed", "Doomed", "bye");

    let (stdout, _, code) = env.triple(&["rm", "doomed", "--force"]);
    assert_eq!(code, 0);
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(payload["deleted"], true);

    let (_, stderr, code) = env.triple(&["doomed"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("not_found"));
}

#[test]
fn alias_add_use_unalias_lifecycle() {
    let env = ContractEnv::new();
    env.seed_prompt("aliased", "Aliased", "content");

    let (stdout, _, code) = env.triple(&["alias", "aliased", "al", "zz"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("aliased"));

    // Use it via direct mode.
    let (stdout, _, code) = env.triple(&["al"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "content\n");

    // Remove alias.
    let (_, _, code) = env.triple(&["unalias", "al"]);
    assert_eq!(code, 0);

    // Alias no longer resolves via ALIAS step — but the canonical id
    // 'aliased' still resolves through PREFIX ('al' is its unique prefix).
    // Resolution-engine behavior: prefix hit returns content again.
    let (stdout, stderr, code) = env.triple(&["al"]);
    assert_eq!(code, 0, "unique prefix must still resolve: {stderr}");
    assert_eq!(stdout, "content\n");
    // 'zz' alias still exists (only 'al' was removed) → resolves via ALIAS.
    let (stdout, _, code) = env.triple(&["zz"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "content\n");

    // A truly unknown query does not resolve.
    let (_, stderr, code) = env.triple(&["totally-unknown-xyz"]);
    assert!(stderr.contains("not_found"), "stderr={stderr}");
    assert_eq!(code, 1);
}

#[test]
fn alias_conflict_returns_error() {
    let env = ContractEnv::new();
    env.seed_prompt("one", "One", "1");
    env.seed_prompt("two", "Two", "2");

    // Alias shadowing a canonical id → id_conflict
    let (_, _, code) = env.triple(&["alias", "two", "one"]);
    assert_eq!(code, 1);

    // Duplicate alias across prompts → conflict on second assignment
    env.seed_prompt("three", "Three", "3");
    let (_, _, code) = env.triple(&["alias", "one", "shared"]);
    assert_eq!(code, 0);
    let (_, _, code) = env.triple(&["alias", "two", "shared"]);
    assert_eq!(code, 1);
}
