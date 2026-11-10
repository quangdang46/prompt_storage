//! Contract tests for bead prompt_storage-p0-contract-harness-pst-functional-batch-n5h.
//!
//! Covers the functional fixes: collection `list` verb, `search -l 0`,
//! variable-name validation + required marker, title/content validation,
//! bounded stdin, and render/copy --fill contract.

mod contract_support;
use contract_support::ContractEnv;
use std::io::Write;
use std::process::Stdio;

#[test]
fn collection_list_verb_lists_collections() {
    let env = ContractEnv::new();
    let (_out, _err, code) = env.triple(&["collection", "list"]);
    assert_eq!(code, 0);
    let (out, _err, code) = env.triple(&["collection", "list", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v.get("collections").is_some());
}

#[test]
fn search_limit_zero_returns_empty() {
    let env = ContractEnv::new();
    env.seed_prompt("alpha", "Alpha", "searchable content");
    let (out, _err, code) = env.triple(&["search", "alpha", "-l", "0", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"], 0);
    assert_eq!(v["results"].as_array().unwrap().len(), 0);
}

#[test]
fn digit_leading_and_pure_underscore_var_names_rejected() {
    let env = ContractEnv::new();
    // Use the command builder so we can attach stdin.
    for bad in ["9X=text", "_9=text", "___=text", "9=text"] {
        let mut cmd = env.command(&["new", "vtest", "--var", bad, "--from", "-"]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"content {{X}}")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code().unwrap(), 1, "var {bad} must fail");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("invalid_var_name"), "stderr: {err}");
    }
}

#[test]
fn required_var_marker_enforced_with_missing_variables_error() {
    let env = ContractEnv::new();
    let mut cmd = env.command(&[
        "new",
        "reqp",
        "--var",
        "NEED!=text",
        "--var",
        "OPT=text=def",
        "--from",
        "-",
    ]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"need {{NEED}} opt {{OPT}}")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 0);

    // Missing required → missing_variables naming NEED (OPT has default).
    let (_o, err, code) = env.triple(&["render", "reqp"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "missing_variables");
    assert_eq!(v["missing"], serde_json::json!(["NEED"]));

    // Explicit --NEED fills it; OPT falls back to default.
    let (out, _e, code) = env.triple(&["render", "reqp", "--NEED=x"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "need x opt def");

    // --fill on non-TTY → tty_required.
    let (_o, err, code) = env.triple(&["render", "reqp", "--fill"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "tty_required");
    assert_eq!(v["missing"], serde_json::json!(["NEED"]));
}

#[test]
fn title_and_content_validation() {
    let env = ContractEnv::new();
    // Newline in title → invalid_title.
    let mut cmd = env.command(&["new", "t1", "--title", "a\nb", "--from", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid_title"));

    // NUL byte in content → invalid_content.
    let mut cmd = env.command(&["new", "t2", "--from", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(b"a\0b").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid_content"));

    // Overlong title → title_too_long.
    let long_title = "t".repeat(2000);
    let mut cmd = env.command(&["new", "t3", "--title", &long_title, "--from", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("title_too_long"));

    // Overlong id → invalid_id.
    let long_id = format!("{}-{}", "a".repeat(100), "b".repeat(100));
    let mut cmd = env.command(&["new", &long_id, "--from", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid_id"));
}

#[test]
fn stdin_content_is_bounded() {
    let env = ContractEnv::new();
    // 1 MiB + 1 byte of content must be rejected without hanging/OOM.
    let payload = vec![b'x'; 1_048_577];
    let mut cmd = env.command(&["new", "big", "--from", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code().unwrap(), 1);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("content_too_large") || err.contains("invalid_data"),
        "stderr: {err}"
    );
}
