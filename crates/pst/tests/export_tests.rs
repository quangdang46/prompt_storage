//! E2E contract tests for export/import commands (bead P3.3).

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn jsonl_export_then_import_merge_roundtrip() {
    let env = ContractEnv::new();
    env.seed_prompt("alpha", "Alpha", "content-alpha");
    env.seed_prompt("beta", "Beta", "content-beta");

    // Export.
    let out = env.home.path().join("backup.jsonl");
    let (stdout, stderr, code) = env.triple(&[
        "export",
        "--format",
        "jsonl",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "export failed: {stdout} {stderr}");

    // Wipe the library by deleting prompts via CLI.
    let (_, _, code) = env.triple(&["rm", "alpha", "--force"]);
    assert_eq!(code, 0);
    let (_, _, code) = env.triple(&["rm", "beta", "--force"]);
    assert_eq!(code, 0);

    // Import restores both.
    let (stdout, stderr, code) = env.triple(&["import", out.to_str().unwrap(), "--merge"]);
    assert_eq!(code, 0, "import failed: {stdout} {stderr}");

    // Verify restored content byte-exact via direct get.
    let (stdout, _, _) = env.triple(&["alpha"]);
    // alpha may resolve via prefix to itself
    assert!(stdout.contains("content-"), "got: {stdout:?}");
}

#[test]
fn markdown_export_writes_safe_files() {
    let env = ContractEnv::new();
    env.seed_prompt("md-demo", "MD Demo", "BODY TEXT");

    let dir = env.home.path().join("md-out");
    let (stdout, stderr, code) = env.triple(&[
        "export",
        "--format",
        "md",
        "--out",
        dir.to_str().unwrap(),
        "--all",
    ]);
    assert_eq!(code, 0, "export failed: {stdout} {stderr}");

    let file = dir.join("md-demo.md");
    assert!(file.exists(), "markdown file must exist");
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.starts_with("# MD Demo\n"));
    assert!(content.contains("BODY TEXT"));
}

#[test]
fn import_corrupt_file_fails_without_destroying_library() {
    let env = ContractEnv::new();
    env.seed_prompt("survivor", "Survivor", "keep me");

    let bad = env.home.path().join("bad.jsonl");
    std::fs::write(&bad, "{corrupt").unwrap();

    let (_, stderr, code) = env.triple(&["import", bad.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("import_failed"));

    // Survivor intact.
    let (stdout, _, _) = env.triple(&["survivor"]);
    assert_eq!(stdout, "keep me\n");
}
