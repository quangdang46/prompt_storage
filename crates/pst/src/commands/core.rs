//! Core command implementations (bead P2.3): new, show, rm, alias, unalias.
//!
//! Presentation logic only — every DB touch goes through `pst::storage`.

use anyhow::Result;
use serde::Serialize;

use crate::model::Prompt;
use crate::storage::database::Database;

/// Max content size accepted via `new --from -` (plan polish R4).
pub const MAX_CONTENT_BYTES: usize = 1_048_576;

/// Error payload for machine consumers; also human-readable on stderr.
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
    pub extra: Vec<(String, serde_json::Value)>,
    pub exit: u8,
}

impl CmdError {
    /// Errors go to STDERR (stdout stays data-only per contract §6).
    pub fn emit(&self) -> i32 {
        let mut obj = serde_json::json!({ "error": self.code, "message": self.message });
        if let Some(map) = obj.as_object_mut() {
            for (k, v) in &self.extra {
                map.insert(k.clone(), v.clone());
            }
        }
        eprintln!("{obj}");
        self.exit as i32
    }
}

/// Options for `pst new` (keeps arg count under clippy's threshold).
pub struct NewArgs<'a> {
    pub id: &'a str,
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub from: Option<String>,
    pub force: bool,
}

/// `pst new <id>` — create a prompt from --from FILE | - (stdin) | editor.
/// Editor paths are TTY-gated (agents can't hang).
pub fn cmd_new(db: &Database, args: NewArgs<'_>) -> Result<i32> {
    // Validate canonical id shape.
    let id = args.id;
    if !is_canonical_id(id) {
        return Ok(CmdError {
            code: "invalid_id",
            message: format!("id must be lowercase kebab-case, got '{}'", id),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    // Duplicate check unless --force.
    if !args.force && db.get_prompt(id)?.is_some() {
        return Ok(CmdError {
            code: "already_exists",
            message: format!("prompt '{id}' already exists (use --force to overwrite)"),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    // Content source resolution.
    let content = match args.from.as_deref() {
        Some("-") => {
            let mut buf = String::new();
            match std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
                Ok(_) => buf,
                Err(e) => {
                    return Ok(CmdError {
                        code: "stdin_read_error",
                        message: e.to_string(),
                        extra: vec![],
                        exit: 1,
                    }
                    .emit());
                }
            }
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(CmdError {
                    code: "file_read_error",
                    message: format!("{path}: {e}"),
                    extra: vec![],
                    exit: 1,
                }
                .emit());
            }
        },
        None => {
            if !atty::is(atty::Stream::Stdin) {
                return Ok(CmdError {
                    code: "tty_required",
                    message: "interactive editing requires a terminal".into(),
                    extra: vec![(
                        "hint".into(),
                        serde_json::json!("pipe content: pst new <id> --from -"),
                    )],
                    exit: 1,
                }
                .emit());
            }
            // Real TTY: read a single line is useless for prompts; direct user
            // to heredoc/stdin in v1 (full $EDITOR flow lands with P3).
            return Ok(CmdError {
                code: "editor_not_available",
                message: "pass content via --from FILE or pipe to --from -".into(),
                extra: vec![],
                exit: 1,
            }
            .emit());
        }
    };

    // Content validation.
    if content.trim().is_empty() {
        return Ok(CmdError {
            code: "empty_content",
            message: "prompt content must not be empty".into(),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }
    if content.len() > MAX_CONTENT_BYTES {
        return Ok(CmdError {
            code: "content_too_large",
            message: "content exceeds limit".into(),
            extra: vec![("max_bytes".into(), MAX_CONTENT_BYTES.into())],
            exit: 1,
        }
        .emit());
    }

    let mut prompt = Prompt::new(
        id,
        args.title.clone().unwrap_or_else(|| id.replace('-', " ")),
        content,
    );
    prompt.description = args.description.clone();
    prompt.category = args.category.clone();
    prompt.tags = args.tags.clone();
    db.upsert_prompt(&prompt)?;
    println!("{}", serde_json::json!({ "created": id }));
    Ok(0)
}

/// `pst show <id>` — human-oriented metadata view (never the piped-content path).
pub fn cmd_show(db: &Database, query: &str, as_json: bool) -> Result<i32> {
    let outcome = crate::storage::resolve::resolve(db, query)?;
    match outcome {
        crate::storage::resolve::ResolveOutcome::Hit { id, .. } => {
            let p = db.get_prompt(&id)?.expect("resolved prompt exists");
            let aliases = db.aliases_for(&p.id)?;
            if as_json {
                let payload = ShowPayload::from_prompt(&p, &aliases);
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{} ({})", p.title, p.id);
                if let Some(d) = &p.description {
                    println!("{d}");
                }
                println!();
                let meta_line = [
                    p.category.as_deref().map(|c| format!("category: {c}")),
                    (!p.tags.is_empty()).then(|| format!("tags: {}", p.tags.join(", "))),
                    (!aliases.is_empty()).then(|| format!("aliases: {}", aliases.join(", "))),
                    Some(format!("used {} times", p.use_count)),
                ];
                for m in meta_line.into_iter().flatten() {
                    println!("{m}");
                }
            }
            Ok(0)
        }
        other => err_forward(other),
    }
}

#[derive(Serialize)]
struct ShowPayload {
    id: String,
    title: String,
    description: Option<String>,
    category: Option<String>,
    tags: Vec<String>,
    aliases: Vec<String>,
    variables: Vec<PromptSummaryVar>,
    use_count: i64,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Serialize)]
struct PromptSummaryVar {
    name: String,
    #[serde(rename = "type")]
    var_type: String,
    required: bool,
    default: Option<String>,
}

impl ShowPayload {
    fn from_prompt(p: &Prompt, aliases: &[String]) -> Self {
        Self {
            id: p.id.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
            category: p.category.clone(),
            tags: p.tags.clone(),
            aliases: aliases.to_vec(),
            variables: p
                .variables
                .iter()
                .map(|v| PromptSummaryVar {
                    name: v.name.clone(),
                    var_type: v.var_type.as_str().to_string(),
                    required: v.required,
                    default: v.default.clone(),
                })
                .collect(),
            use_count: p.use_count,
            created_at: p.created_at.clone(),
            updated_at: p.updated_at.clone(),
        }
    }
}

/// `pst rm <id>` — delete with confirm-on-TTY / --force bypass.
pub fn cmd_rm(db: &Database, id: &str, force: bool) -> Result<i32> {
    // Resolve first so aliases work too.
    let outcome = crate::storage::resolve::resolve(db, id)?;
    let target = match outcome {
        crate::storage::resolve::ResolveOutcome::Hit { id, source, .. } => {
            // Exact-id deletion only when explicitly forced or exact source;
            // alias/prefix hits still resolve to their owner.
            let _ = source;
            id
        }
        _ => id.to_string(),
    };

    let exists = db.get_prompt(&target)?.is_some();
    if !exists {
        return Ok(CmdError {
            code: "not_found",
            message: format!("no prompt '{target}'"),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    if !force && atty::is(atty::Stream::Stdin) {
        eprint!("Delete '{target}'? [y/N] ");
        use std::io::Write;
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            println!("{}", serde_json::json!({ "deleted": false, "id": target }));
            return Ok(0);
        }
    }

    db.delete_prompt(&target)?;
    println!("{}", serde_json::json!({ "deleted": true, "id": target }));
    Ok(0)
}

/// `pst alias <id> <alias>...`
pub fn cmd_alias(db: &Database, id: &str, aliases: &[String]) -> Result<i32> {
    for alias in aliases {
        if let Err(e) = db.add_alias(alias, id) {
            let code = match e.to_string().as_str() {
                "id_conflict" => "id_conflict",
                "alias_conflict" => "alias_conflict",
                "not_found" => "not_found",
                _ => "alias_error",
            };
            return Ok(CmdError {
                code,
                message: format!("cannot alias '{alias}' -> '{id}'"),
                extra: vec![("alias".into(), alias.clone().into())],
                exit: 1,
            }
            .emit());
        }
        println!("{}", serde_json::json!({ "aliased": alias, "to": id }));
    }
    Ok(0)
}

/// `pst unalias <alias>...`
pub fn cmd_unalias(db: &Database, aliases: &[String]) -> Result<i32> {
    for alias in aliases {
        let removed = db.remove_alias(alias)?;
        if !removed {
            return Ok(CmdError {
                code: "not_found",
                message: format!("no such alias '{alias}'"),
                extra: vec![],
                exit: 1,
            }
            .emit());
        }
        println!("{}", serde_json::json!({ "unaliased": alias }));
    }
    Ok(0)
}

fn err_forward(other: crate::storage::resolve::ResolveOutcome) -> Result<i32> {
    match other {
        crate::storage::resolve::ResolveOutcome::Ambiguous { query, candidates } => {
            let ids: Vec<serde_json::Value> = candidates
                .iter()
                .map(|c| serde_json::json!({"id": c.id, "title": c.title}))
                .collect();
            eprintln!(
                "{}",
                serde_json::json!({ "error": "ambiguous", "query": query, "candidates": ids })
            );
            Ok(1)
        }
        crate::storage::resolve::ResolveOutcome::NotFound { query } => {
            eprintln!(
                "{}",
                serde_json::json!({ "error": "not_found", "query": query })
            );
            Ok(1)
        }
        _ => unreachable!("err_forward called with Hit"),
    }
}

/// Canonical id check: ^[a-z][a-z0-9]*(-[a-z0-9]+)*$
pub fn is_canonical_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut prev_hyphen = false;
    for c in chars {
        match c {
            '-' => {
                if prev_hyphen {
                    return false;
                }
                prev_hyphen = true;
            }
            c if c.is_ascii_lowercase() || c.is_ascii_digit() => prev_hyphen = false,
            _ => return false,
        }
    }
    !prev_hyphen
}
