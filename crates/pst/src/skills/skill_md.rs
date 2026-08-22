//! Integration-skill generator (bead P6.1): one `prompt-storage` skill.
//!
//! The five behavioral rules are frozen verbatim (plan §11) and asserted by
//! a golden test. Template lives in a const — zero runtime file deps.

use sha2::{Digest, Sha256};

pub const SKILL_NAME: &str = "prompt-storage";

/// The five behavioral rules, verbatim from the plan.
pub const BEHAVIORAL_RULES: &str = "\
When a task would benefit from a reusable prompt or workflow:
1. Search pst first (`pst search` / `pst suggest`).
2. Prefer an existing relevant prompt over recreating one.
3. Retrieve it with `pst <id>`; output is the raw prompt on stdout.
4. Use `pst render <id> --VAR=value` when variables/context are required.
5. Never guess when resolution is ambiguous - ask the user or refine the query.";

/// Generate the full SKILL.md content for the prompt-storage integration skill.
pub fn generate_skill_md() -> String {
    format!(
        r#"---
name: {name}
description: Local prompt registry available via the pst CLI. Teaches agents how to find, retrieve, and render reusable prompts instead of recreating them.
---

# {title}

`pst` is a local, DB-backed personal prompt library. It stores reusable
prompts in SQLite with full-text search and variable templating.

## When to use

{rules}

**Conditional reuse, not reflexive search:** the rules above are gated on
"would benefit". Do NOT run `pst search` on every request - weigh latency
and token cost; reach for pst only when a reusable prompt or workflow
plausibly exists.

## Cheatsheet

```bash
pst <id>                      # raw prompt on stdout (exact/alias/prefix/fuzzy)
pst search "<terms>"          # ranked full-text search
pst suggest "<task>"          # recommendations with reasons
pst render <id> --VAR=value   # substitute variables into the prompt
pst show <id>                 # metadata + preview
```

## Notes

- Direct output (`pst <id>`) is raw content on stdout - safe to pipe.
- If pst reports ambiguity, ask the user or refine the query; it never
  guesses silently.
- `PST_HOME` relocates the library root (per-project or per-user scoping).
"#,
        name = SKILL_NAME,
        title = "Prompt Storage",
        rules = BEHAVIORAL_RULES,
    )
}

/// sha256 hex of content — used for idempotent install checks.
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_contains_all_five_rules() {
        let md = generate_skill_md();
        for fragment in [
            "Search pst first",
            "Prefer an existing relevant prompt over recreating one.",
            "Retrieve it with `pst <id>`",
            "Use `pst render <id> --VAR=value`",
            "Never guess when resolution is ambiguous",
        ] {
            let normalized = fragment.replace('`', "");
            assert!(
                md.contains(fragment) || md.contains(&normalized),
                "rule missing: {fragment}"
            );
        }
    }

    #[test]
    fn skill_has_frontmatter_and_anti_pattern_note() {
        let md = generate_skill_md();
        assert!(md.starts_with("---\nname: prompt-storage\n"));
        assert!(md.contains("Do NOT run `pst search` on every request"));
        assert!(md.contains("PST_HOME"));
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(generate_skill_md(), generate_skill_md());
        assert_eq!(
            sha256_hex(&generate_skill_md()),
            sha256_hex(&generate_skill_md())
        );
    }
}
