//! Agent adapters + install/uninstall (bead P6.2).
//!
//! Canonical copy ALWAYS lands first in `<root>/.agents/skills/prompt-storage/`.
//! Adapters symlink (or copy) it into their own skill dirs. Adapter failures
//! are reported, never fatal.
//!
//! UNINSTALL INVARIANT (locked): removes ONLY the integration skill — never
//! any prompt, collection, or database row.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::skill_md::{SKILL_NAME, generate_skill_md, sha256_hex};

/// One agent adapter definition.
#[derive(Debug, Clone, Copy)]
pub struct AgentAdapter {
    pub id: &'static str,
    /// Subdirectory under root for this agent's skills.
    pub dir: &'static str,
}

/// Registered adapters. Nothing hardcoded outside this list.
pub const ADAPTERS: &[AgentAdapter] = &[
    AgentAdapter {
        id: "claude",
        dir: ".claude/skills",
    },
    AgentAdapter {
        id: "codex",
        dir: ".codex/skills",
    },
];

/// Resolve the install root.
/// - `--project`: walk up from cwd looking for `.git` or `.agents` (repo
///   root), like auto-detect — running from a subdirectory must not
///   install the skill into the subdir. Falls back to cwd.
/// - `--personal`: $HOME (PST_HOME-aware).
/// - default: walk up for `.git`, then `.agents`, else $HOME.
pub fn resolve_root(project: bool, personal: bool) -> PathBuf {
    if personal {
        return dirs_home();
    }
    if project {
        if let Some(root) = find_project_root() {
            return root;
        }
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    // Auto-detect: walk up for .git, then .agents
    if let Some(root) = find_project_root() {
        return root;
    }
    dirs_home()
}

/// Walk up from cwd looking for `.git` or `.agents`; None at filesystem root.
fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut probe = cwd;
    loop {
        if probe.join(".git").exists() || probe.join(".agents").exists() {
            return Some(probe);
        }
        probe = probe.parent()?.to_path_buf();
    }
}

fn dirs_home() -> PathBuf {
    // PST_HOME takes precedence (test isolation + per-project scoping).
    if let Some(h) = std::env::var_os("PST_HOME") {
        return PathBuf::from(h);
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn canonical_dir(root: &Path) -> PathBuf {
    root.join(".agents/skills").join(SKILL_NAME)
}

pub struct InstallReport {
    pub canonical: PathBuf,
    pub installed: Vec<&'static str>,
    pub skipped: Vec<String>,
}

/// Install the integration skill. Idempotent via sha256 of generated content.
pub fn install(root: &Path, force: bool) -> Result<InstallReport> {
    let content = generate_skill_md();
    let new_hash = sha256_hex(&content);

    let canon = canonical_dir(root);
    std::fs::create_dir_all(&canon).with_context(|| format!("creating {}", canon.display()))?;
    let skill_file = canon.join("SKILL.md");

    // Idempotency: unchanged content → no-op rewrite (but still wire links).
    if !force && skill_file.exists() && same_content(&skill_file, new_hash) {
        // Content identical; ensure links anyway (cheap, idempotent).
        // Adapter failures are collected, never fatal (contract).
        let (installed, skipped) = wire_all_adapters(root, &canon, &content);
        return Ok(InstallReport {
            canonical: skill_file,
            installed,
            skipped,
        });
    }

    std::fs::write(&skill_file, &content)
        .with_context(|| format!("writing {}", skill_file.display()))?;

    let (installed, skipped) = wire_all_adapters(root, &canon, &content);
    Ok(InstallReport {
        canonical: skill_file,
        installed,
        skipped,
    })
}

/// Wire every adapter; failures land in `skipped` — NEVER abort the
/// install (documented contract: "adapter failures are reported, never
/// fatal").
fn wire_all_adapters(root: &Path, canon: &Path, content: &str) -> (Vec<&'static str>, Vec<String>) {
    let mut installed = Vec::new();
    let mut skipped = Vec::new();
    for adapter in ADAPTERS {
        match wire_adapter(root, adapter, canon, content) {
            Ok(true) => installed.push(adapter.id),
            Ok(false) => {}
            Err(e) => skipped.push(format!("{}: {}", adapter.id, e)),
        }
    }
    (installed, skipped)
}

/// Compare file on disk to fresh hash.
fn same_content(file: &Path, expected_hash: String) -> bool {
    std::fs::read_to_string(file)
        .map(|existing| sha256_hex(&existing) == expected_hash)
        .unwrap_or(false)
}

/// Create/refresh the link for one adapter. Returns true when wired now,
/// false when already correct or environment absent.
fn wire_adapter(root: &Path, adapter: &AgentAdapter, canon: &Path, content: &str) -> Result<bool> {
    let target_dir = root.join(adapter.dir).join(SKILL_NAME);
    let _ = std::fs::create_dir_all(target_dir.parent().unwrap());

    #[cfg(unix)]
    {
        // Symlink needs no content; silence unused param on unix builds.
        let _ = content;
        // Compute relative path from link location to canonical.
        let link = target_dir.clone();
        if link.exists() || link.is_symlink() {
            // Already there? verify it resolves to canonical.
            if let Ok(resolved) = link.canonicalize() {
                let canon_abs = canon.canonicalize().unwrap_or_else(|_| canon.to_path_buf());
                if resolved == canon_abs {
                    return Ok(false);
                }
            }
            let _ = std::fs::remove_file(&link);
        }
        // Relative symlink ../../.agents/skills/prompt-storage
        let rel = compute_relative(&link, canon)?;
        std::os::unix::fs::symlink(&rel, &link)
            .with_context(|| format!("symlinking {} -> {}", link.display(), rel))?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        // Copy fallback on filesystems without symlink support.
        let _ = canon;
        let file = target_dir.join("SKILL.md");
        let needs = !file.exists()
            || std::fs::read_to_string(&file)
                .map(|e| e != content)
                .unwrap_or(true);
        if needs {
            std::fs::create_dir_all(&target_dir)?;
            std::fs::write(&file, content)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(unix)]
fn compute_relative(link: &Path, canon: &Path) -> Result<String> {
    let link_parent = link.parent().context("link has no parent")?;
    let link_depth = link_parent.components().count();
    let canon_components: Vec<_> = canon.components().collect();
    let link_parent_comps: Vec<_> = link_parent.components().collect();

    // Common prefix length.
    let common = link_parent_comps
        .iter()
        .zip(canon_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Relative path from link_parent up to the common ancestor, then down
    // to the canonical dir.
    let mut up = String::new();
    for _ in common..link_depth {
        up.push_str("../");
    }
    let down: String = canon_components[common..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("{up}{down}"))
}

/// Ownership checks for uninstall:
/// - a LINK is pst's iff it resolves to the canonical dir
/// - a COPIED FILE is pst's iff byte-identical to regenerated content
fn owned_link(link: &Path, canon: &Path) -> bool {
    if !link.is_symlink() {
        return false;
    }
    link.canonicalize()
        .map(|r| r == canon.canonicalize().unwrap_or_else(|_| canon.to_path_buf()))
        .unwrap_or(false)
}

/// Expected file set inside an adapter skill dir installed by COPY mode:
/// exactly one SKILL.md whose bytes equal the generated skill.
fn owned_copy_dir(dir: &Path, content: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut saw_skill = false;
    for e in entries.flatten() {
        let name = e.file_name();
        if name == "SKILL.md" {
            saw_skill = true;
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                return false;
            };
            if text != content {
                return false; // user-edited copy — not ours to delete
            }
        } else {
            return false; // user-added file inside the skill dir — keep it
        }
    }
    saw_skill
}

/// Remove `dir` only if it is now empty; never fails, never recursive.
fn prune_empty_dir(dir: &Path) {
    let _ = std::fs::remove_dir(dir); // fails harmlessly when non-empty
}

/// Uninstall: remove ONLY pst-owned artifacts. Never touches the DB and
/// never deletes user data: a copied skill dir is removed only when its
/// contents are EXACTLY the expected SKILL.md; the canonical dir is
/// removed only when its SKILL.md still matches generated content.
pub fn uninstall(root: &Path) -> Result<usize> {
    let content = generate_skill_md();
    let canon = canonical_dir(root);
    let mut removed = 0usize;

    for adapter in ADAPTERS {
        let link = root.join(adapter.dir).join(SKILL_NAME);
        if link.is_symlink() && owned_link(&link, &canon) {
            let parent = link.parent().map(Path::to_path_buf);
            let _ = std::fs::remove_file(&link);
            removed += 1;
            // Prune the (now possibly empty) skill container dirs we made.
            if let Some(p) = parent {
                prune_empty_dir(&p);
                if let Some(gp) = p.parent() {
                    prune_empty_dir(gp);
                }
            }
        } else if link.is_dir() && owned_copy_dir(&link, &content) {
            let parent = link.parent().map(Path::to_path_buf);
            let _ = std::fs::remove_dir_all(&link);
            removed += 1;
            if let Some(p) = parent {
                prune_empty_dir(&p);
                if let Some(gp) = p.parent() {
                    prune_empty_dir(gp);
                }
            }
        }
        // Anything else (user files, foreign content) is left untouched.
    }

    // Canonical dir: remove only when it contains EXACTLY our generated
    // SKILL.md and nothing else. Anything more (user edits, extra files)
    // is user data and stays.
    let canon_skill = canon.join("SKILL.md");
    if canon.is_dir() {
        let exact_set = std::fs::read_dir(&canon)
            .map(|entries| {
                let names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
                names.len() == 1 && names[0] == "SKILL.md"
            })
            .unwrap_or(false);
        let owned = std::fs::read_to_string(&canon_skill)
            .map(|t| t == content)
            .unwrap_or(false);
        if exact_set && owned {
            std::fs::remove_dir_all(&canon)
                .with_context(|| format!("removing {}", canon.display()))?;
            removed += 1;
        }
        // Otherwise the canonical dir holds user data — keep it.
    }
    Ok(removed)
}
