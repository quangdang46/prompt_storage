//! Export/import command layer (bead P3.3): markdown export, path safety.
//!
//! JSONL core already exists in storage::jsonl; this module adds the CLI
//! surface plus markdown formatting with traversal-safe filenames.

use std::path::Path;

use anyhow::{Context, Result};

use crate::model::Prompt;
use crate::storage::database::Database;
use crate::storage::jsonl::{export_jsonl as core_export, import_jsonl as core_import};

pub use crate::storage::jsonl::ImportMode;

/// Max content bytes accepted for `new`/imported content (plan polish R4).
pub const MAX_CONTENT_BYTES: usize = 1_048_576;

/// Derive a filesystem-safe filename from a prompt id: `^[a-z0-9-]+$`
/// enforced after derivation — anything else is rejected (exit-2 class).
pub fn safe_filename(prompt_id: &str, ext: &str) -> Result<String> {
    let cleaned: String = prompt_id
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    if cleaned != prompt_id || cleaned.is_empty() {
        anyhow::bail!("unsafe id");
    }
    Ok(format!("{cleaned}.{ext}"))
}

/// Ensure `candidate` resolves inside `root` (no path traversal).
pub fn ensure_within(root: &Path, candidate: &Path) -> Result<()> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Build absolute candidate relative to root when relative.
    let abs = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        canonical_root.join(candidate)
    };
    // Walk up until we find an existing ancestor, then verify prefix.
    let mut probe = abs.clone();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    let resolved = probe.canonicalize().unwrap_or_else(|_| probe.to_path_buf());
    if !resolved.starts_with(&canonical_root) {
        anyhow::bail!("unsafe path");
    }
    Ok(())
}

/// Format one prompt as markdown (plan §8 export format).
pub fn format_markdown(p: &Prompt) -> String {
    let mut out = format!("# {}\n\n", p.title);
    if let Some(d) = &p.description {
        out.push_str(&format!("{d}\n\n"));
    }
    if let Some(c) = &p.category {
        out.push_str(&format!("**Category**: {c}\n\n"));
    }
    if !p.tags.is_empty() {
        out.push_str(&format!("**Tags**: {}\n\n", p.tags.join(", ")));
    }
    out.push_str("---\n\n");
    out.push_str(&p.content);
    out.push('\n');
    out
}

/// Export prompts as markdown files into `dir`. Returns written paths.
pub fn export_markdown_all(db: &Database, dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    ensure_within(dir, Path::new(""))?;
    let prompts = db.list_prompts_filtered(None, None, false)?;
    let mut written = Vec::new();
    for p in &prompts {
        let name = safe_filename(&p.id, "md")?;
        let target = dir.join(name);
        ensure_within(dir, &target)?;
        std::fs::write(&target, format_markdown(p))
            .with_context(|| format!("writing {}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}

/// Re-export core JSONL functions under a unified API for main.rs dispatch.
pub fn export_jsonl(db: &Database, path: &Path) -> Result<usize> {
    core_export(db, path)
}

pub fn import_jsonl(db: &Database, path: &Path, mode: ImportMode) -> Result<usize> {
    core_import(db, path, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_accepts_kebab() {
        assert_eq!(
            safe_filename("code-review", "md").unwrap(),
            "code-review.md"
        );
    }

    #[test]
    fn safe_filename_rejects_uppercase_and_dots() {
        assert!(safe_filename("Bad_ID", "md").is_err());
        assert!(safe_filename("../escape", "md").is_err());
        assert!(safe_filename("", "md").is_err());
    }

    #[test]
    fn ensure_within_rejects_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let inside = tmp.path().join("sub/file.md");
        ensure_within(tmp.path(), &inside).unwrap(); // fine
        let outside = Path::new("/etc/passwd");
        assert!(ensure_within(tmp.path(), outside).is_err());
    }

    #[test]
    fn markdown_format_includes_metadata_and_content() {
        let mut p = Prompt::new("demo", "Demo Title", "BODY");
        p.description = Some("desc".into());
        p.category = Some("testing".into());
        p.tags = vec!["t1".into()];
        let md = format_markdown(&p);
        assert!(md.starts_with("# Demo Title\n\n"));
        assert!(md.contains("desc\n\n"));
        assert!(md.contains("**Category**: testing"));
        assert!(md.contains("**Tags**: t1"));
        assert!(md.contains("---\n\nBODY\n"));
    }

    #[test]
    fn export_markdown_writes_one_file_per_prompt() {
        let db = Database::in_memory().unwrap();
        db.upsert_prompt(&Prompt::new("aa", "AA", "x")).unwrap();
        db.upsert_prompt(&Prompt::new("bb", "BB", "y")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let files = export_markdown_all(&db, tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
        for f in &files {
            assert!(f.exists());
            assert!(f.extension().unwrap() == "md");
        }
    }
}
