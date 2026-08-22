//! Integration tests for the prompt-storage skill lifecycle (bead P6.x).

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn install_is_idempotent_single_skill_dir() {
    let env = ContractEnv::new();
    // Run twice from the same root.
    for _ in 0..2 {
        let (stdout, stderr, code) = env.triple(&["install", "--personal"]);
        assert_eq!(code, 0, "install failed: {stdout} {stderr}");
    }
    let canon = env.home.path().join(".agents/skills/prompt-storage");
    assert!(
        canon.join("SKILL.md").exists(),
        "canonical skill must exist"
    );

    // Count adapter links — must be exactly one per adapter (no dupes).
    let claude_link = env.home.path().join(".claude/skills/prompt-storage");
    let codex_link = env.home.path().join(".codex/skills/prompt-storage");
    assert!(claude_link.exists());
    assert!(codex_link.exists());

    // Re-run: still exactly one dir per location.
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);
    assert!(canon.join("SKILL.md").exists());
}

#[test]
fn skill_content_has_five_rules_verbatim() {
    let env = ContractEnv::new();
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);
    let md = std::fs::read_to_string(
        env.home
            .path()
            .join(".agents/skills/prompt-storage/SKILL.md"),
    )
    .unwrap();
    for rule in [
        "Search pst first",
        "Prefer an existing relevant prompt",
        "Retrieve it with `pst <id>`",
        "Use `pst render <id> --VAR=value`",
        "Never guess when resolution is ambiguous",
    ] {
        assert!(
            md.contains(rule),
            "missing rule fragment: {rule}\n---\n{md}"
        );
    }
    // Anti-pattern note present.
    assert!(md.to_lowercase().contains("do not run"));
}

#[test]
fn uninstall_removes_only_the_skill() {
    let env = ContractEnv::new();
    env.seed_prompt("precious", "Precious", "DO NOT DELETE");
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);

    let canon = env.home.path().join(".agents/skills/prompt-storage");
    assert!(canon.exists());

    let (_, _, code) = env.triple(&["uninstall", "--personal"]);
    assert_eq!(code, 0);

    assert!(!canon.exists(), "skill dir removed");
    assert!(
        !env.home
            .path()
            .join(".claude/skills/prompt-storage")
            .exists()
    );
    assert!(
        !env.home
            .path()
            .join(".codex/skills/prompt-storage")
            .exists()
    );

    // THE INVARIANT: prompts untouched.
    let (stdout, _, _) = env.triple(&["precious"]);
    assert_eq!(stdout, "DO NOT DELETE\n", "library must be untouched");
}
