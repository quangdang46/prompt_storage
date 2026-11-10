//! Contract regression tests for skills install/uninstall beads:
//! - uninstall-deletes-user-data-2cf: uninstall never deletes user data.
//! - install-adapter-fatal-project-v8l: adapter failures are reported in
//!   'skipped', never fatal; --project resolves the repo root.

mod contract_support;

use contract_support::ContractEnv;
use std::path::Path;

fn skill_md(root: &Path) -> std::path::PathBuf {
    root.join(".agents/skills/prompt-storage/SKILL.md")
}

#[test]
fn uninstall_preserves_user_edited_canonical_skill() {
    let env = ContractEnv::new();
    let home = env.home.path().to_path_buf();
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);
    assert!(skill_md(&home).exists());

    // User edits the canonical SKILL.md — it is THEIR data now.
    std::fs::write(skill_md(&home), "user customized\n").unwrap();
    let (_, _, code) = env.triple(&["uninstall", "--personal"]);
    assert_eq!(code, 0);
    assert!(
        skill_md(&home).exists(),
        "user-edited canonical SKILL.md must survive uninstall"
    );
}

#[test]
fn uninstall_preserves_copied_dir_with_user_files() {
    let env = ContractEnv::new();
    let home = env.home.path().to_path_buf();
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);

    // Simulate a COPY-mode adapter dir (real dir, not symlink) containing
    // our SKILL.md plus a user-added file.
    let link = home.join(".claude/skills/prompt-storage");
    if link.is_symlink() {
        std::fs::remove_file(&link).unwrap();
    }
    let copy_dir = home.join(".claude/skills/prompt-storage");
    std::fs::create_dir_all(&copy_dir).unwrap();
    std::fs::copy(skill_md(&home), copy_dir.join("SKILL.md")).unwrap();
    std::fs::write(copy_dir.join("my-notes.txt"), "user data").unwrap();

    let (_, _, code) = env.triple(&["uninstall", "--personal"]);
    assert_eq!(code, 0);
    assert!(
        copy_dir.join("my-notes.txt").exists(),
        "user files inside a copied skill dir must never be deleted"
    );
}

#[test]
fn uninstall_leaves_no_empty_skill_dirs() {
    let env = ContractEnv::new();
    let home = env.home.path().to_path_buf();
    let (_, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0);
    let (_, _, code) = env.triple(&["uninstall", "--personal"]);
    assert_eq!(code, 0);
    // pst-owned skill dirs are gone entirely.
    assert!(!home.join(".agents/skills/prompt-storage").exists());
    assert!(!home.join(".claude/skills/prompt-storage").exists());
    assert!(!home.join(".codex/skills/prompt-storage").exists());
}

#[test]
fn install_adapter_failure_is_skipped_not_fatal() {
    let env = ContractEnv::new();
    let home = env.home.path().to_path_buf();
    // Block the claude adapter with a real directory so symlinking fails.
    let blocker = home.join(".claude/skills/prompt-storage");
    std::fs::create_dir_all(&blocker).unwrap();
    std::fs::write(blocker.join("SKILL.md"), "user content\n").unwrap();

    let (stdout, _, code) = env.triple(&["install", "--personal"]);
    assert_eq!(code, 0, "adapter failure must not be fatal: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["installed"], true);
    assert!(
        v["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().starts_with("claude"))
    );
    // Canonical skill still installed.
    assert!(skill_md(&home).exists());
    // User blocker untouched.
    assert_eq!(
        std::fs::read_to_string(blocker.join("SKILL.md")).unwrap(),
        "user content\n"
    );
}

#[test]
fn install_project_resolves_repo_root_from_subdir() {
    let env = ContractEnv::new();
    let repo = env.home.path().join("repo");
    let sub = repo.join("crates/deep/sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .output()
        .expect("git init");

    // Run `pst install --project` with cwd inside the subdir.
    let out = env
        .command(&["install", "--project"])
        .current_dir(&sub)
        .output()
        .expect("spawn");
    assert!(out.status.success());
    // Skill must land at the REPO root, not the subdir.
    assert!(repo.join(".agents/skills/prompt-storage/SKILL.md").exists());
    assert!(!sub.join(".agents").exists());
}
