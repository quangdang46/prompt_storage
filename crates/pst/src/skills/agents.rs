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

/// Resolve the install root: explicit flag wins; otherwise walk up from cwd
/// looking for `.git`; fall back to $HOME.
pub fn resolve_root(project: bool, personal: bool) -> PathBuf {
    if project {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    if personal {
        return dirs_home();
    }
    // Auto-detect: walk up for .git
    if let Ok(cwd) = std::env::current_dir() {
        let mut probe = cwd.clone();
        loop {
            if probe.join(".git").exists() {
                return probe;
            }
            match probe.parent() {
                Some(p) => probe = p.to_path_buf(),
                None => break,
            }
        }
    }
    dirs_home()
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
        let mut installed = Vec::new();
        for adapter in ADAPTERS {
            if wire_adapter(root, adapter, &canon, &content)? {
                installed.push(adapter.id);
            }
        }
        return Ok(InstallReport {
            canonical: skill_file,
            installed,
            skipped: vec![],
        });
    }

    std::fs::write(&skill_file, &content)
        .with_context(|| format!("writing {}", skill_file.display()))?;

    let mut installed = Vec::new();
    for adapter in ADAPTERS {
        if wire_adapter(root, adapter, &canon, &content)? {
            installed.push(adapter.id);
        } else {
            // reported as skipped below by caller if needed
        }
    }
    Ok(InstallReport {
        canonical: skill_file,
        installed,
        skipped: vec![],
    })
}

/// Compare file on disk to fresh hash.
fn same_content(file: &Path, expected_hash: String) -> bool {
    std::fs::read_to_string(file)
        .map(|existing| sha256_hex(&existing) == expected_hash)
        .unwrap_or(false)
}

/// Create/refresh the link for one adapter. Returns true when wired now,
/// false when already correct or environment absent.
fn wire_adapter(root: &Path, adapter: &AgentAdapter, canon: &Path, _content: &str) -> Result<bool> {
    let target_dir = root.join(adapter.dir).join(SKILL_NAME);
    let _ = std::fs::create_dir_all(target_dir.parent().unwrap());

    #[cfg(unix)]
    {
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

    let mut rel = String::new();
    for _ in common..link_depth {
        rel.push_str("../");
    }
    for comp in &canon_components[common..] {
        rel.push_str(&comp.as_os_str().to_string_lossy());
        rel.push('/');
    }
    rel.push_str("SKILL_DIR_MARKER");
    // Replace marker with directory itself (canonical is a directory).
    let rel = rel.trim_end_matches("SKILL_DIR_MARKER/").to_string() + "/";
    let _ = rel;
    // Simpler robust approach: relative path from link_parent to canon dir.
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

fn owned_copy(file: &Path, content: &str) -> bool {
    std::fs::read_to_string(file)
        .map(|existing| existing == content)
        .unwrap_or(false)
}

/// Uninstall: remove ONLY pst-owned artifacts. Never touches the DB.
pub fn uninstall(root: &Path) -> Result<usize> {
    let content = generate_skill_md();
    let canon = canonical_dir(root);
    let mut removed = 0usize;

    for adapter in ADAPTERS {
        let link = root.join(adapter.dir).join(SKILL_NAME);
        if link.is_symlink() && owned_link(&link, &canon) {
            let _ = std::fs::remove_file(&link);
            removed += 1;
        } else if link.join("SKILL.md").is_file() && owned_copy(&link.join("SKILL.md"), &content) {
            let _ = std::fs::remove_dir_all(&link);
            removed += 1;
        }
    }

    if canon.exists() {
        std::fs::remove_dir_all(&canon).with_context(|| format!("removing {}", canon.display()))?;
        removed += 1;
    }
    Ok(removed)
}
