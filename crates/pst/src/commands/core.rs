//! Core command implementations (bead P2.3): new, show, rm, alias, unalias.
//!
//! Presentation logic only — every DB touch goes through `pst::storage`.

use anyhow::Result;
use serde::Serialize;
use std::io::Read;

use crate::model::Prompt;
use crate::storage::database::Database;

/// Max content size accepted via `new --from -` (plan polish R4).
pub const MAX_CONTENT_BYTES: usize = 1_048_576;
/// Max title length (chars) — titles are metadata, not content.
pub const MAX_TITLE_CHARS: usize = 1024;
/// Max canonical id length (chars).
pub const MAX_ID_CHARS: usize = 128;

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
    /// Declared variables: "NAME=type" or "NAME=type=default"
    pub vars: Vec<String>,
    pub from: Option<String>,
    pub force: bool,
}

/// `pst new <id>` — create a prompt from --from FILE | - (stdin) | editor.
/// Editor paths are TTY-gated (agents can't hang).
pub fn cmd_new(db: &Database, args: NewArgs<'_>) -> Result<i32> {
    // Validate canonical id shape + length.
    let id = args.id;
    if !is_canonical_id(id) || id.chars().count() > MAX_ID_CHARS {
        return Ok(CmdError {
            code: "invalid_id",
            message: format!(
                "id must be lowercase kebab-case (<= {MAX_ID_CHARS} chars), got '{}'",
                id
            ),
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

    // Alias-namespace check: a canonical id equal (NOCASE) to an existing
    // alias would shadow that alias forever — exact-id resolution always
    // wins over ALIAS. `alias` already rejects the reverse direction.
    if db.lookup_alias(id)?.is_some() {
        return Ok(CmdError {
            code: "alias_conflict",
            message: format!("id '{id}' collides with an existing alias"),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    // Content source resolution.
    let content = match args.from.as_deref() {
        Some("-") => {
            // Bounded read: at most MAX_CONTENT_BYTES+1 so a hostile or
            // mistaken infinite stream can't exhaust memory before the
            // size check runs.
            let mut buf = String::new();
            let mut limited = std::io::stdin().take((MAX_CONTENT_BYTES + 1) as u64);
            match std::io::Read::read_to_string(&mut limited, &mut buf) {
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
    if content.contains('\0') {
        return Ok(CmdError {
            code: "invalid_content",
            message: "content must not contain NUL bytes".into(),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    // Title validation: single-line, bounded, no control chars.
    let title = args.title.clone().unwrap_or_else(|| id.replace('-', " "));
    if title.chars().count() > MAX_TITLE_CHARS {
        return Ok(CmdError {
            code: "title_too_long",
            message: format!("title exceeds {MAX_TITLE_CHARS} chars"),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }
    if title.chars().any(|c| c.is_control()) {
        return Ok(CmdError {
            code: "invalid_title",
            message: "title must be a single line without control characters".into(),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }

    let mut prompt = Prompt::new(id, title, content);
    prompt.description = args.description.clone();
    prompt.category = args.category.clone();
    prompt.tags = args.tags.clone();

    // Parse declared variables: --var NAME=type[=default]
    // NAME may end with '!' to mark the variable required.
    // NAME must be usable as a --VAR= token: first char [A-Z_], rest
    // [A-Z0-9_], and at least one letter (so '9X', '___', '9' are rejected
    // — they could never be filled via the CLI anyway).
    for spec in &args.vars {
        let mut parts = spec.splitn(3, '=');
        let Some(raw_name) = parts.next() else {
            continue;
        };
        let (name, required) = match raw_name.strip_suffix('!') {
            Some(base) => (base, true),
            None => (raw_name, false),
        };
        let valid_name = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            && name.chars().any(|c| c.is_ascii_uppercase());
        if !valid_name {
            return Ok(CmdError {
                code: "invalid_var_name",
                message: format!(
                    "variable name must be UPPER_SNAKE_CASE (optionally '!'-suffixed for required), got '{raw_name}'"
                ),
                extra: vec![],
                exit: 1,
            }
            .emit());
        }
        let type_str = parts.next().unwrap_or("text");
        let default = parts.next().map(str::to_string);
        match crate::model::VariableType::from_str_opt(type_str) {
            Some(var_type) => prompt.variables.push(crate::model::PromptVariable {
                name: name.to_string(),
                var_type,
                required,
                description: None,
                default,
            }),
            None => {
                return Ok(CmdError {
                    code: "invalid_var_type",
                    message: format!(
                        "type must be text|multiline|select|file|path, got '{type_str}'"
                    ),
                    extra: vec![],
                    exit: 1,
                }
                .emit());
            }
        }
    }
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
/// Contract: off-TTY (agents/CI) ALWAYS requires --force; on a TTY the
/// user is prompted. Queries resolve like `show` — an ambiguous or
/// unknown query reports `ambiguous`/`not_found`, never a misleading
/// "no prompt".
pub fn cmd_rm(db: &Database, id: &str, force: bool) -> Result<i32> {
    let outcome = crate::storage::resolve::resolve(db, id)?;
    let target = match outcome {
        crate::storage::resolve::ResolveOutcome::Hit { id, .. } => id,
        other => return err_forward(other),
    };

    if !force {
        if atty::is(atty::Stream::Stdin) {
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
        } else {
            // Destructive op with no way to confirm: refuse rather than
            // silently deleting on a piped/agent invocation.
            return Ok(CmdError {
                code: "confirmation_required",
                message: format!(
                    "rm is destructive; pass --force to delete '{target}' non-interactively"
                ),
                extra: vec![],
                exit: 1,
            }
            .emit());
        }
    }

    db.delete_prompt(&target)?;
    println!("{}", serde_json::json!({ "deleted": true, "id": target }));
    Ok(0)
}

/// `pst alias <id> <alias>...` — `<id>` resolves like `show` (exact,
/// alias, prefix, fuzzy), so `pst alias <existing-alias> newname` works.
/// The whole batch is atomic: the first failure rolls back all aliases.
pub fn cmd_alias(db: &Database, id: &str, aliases: &[String]) -> Result<i32> {
    if aliases.is_empty() {
        return Ok(CmdError {
            code: "usage",
            message: "alias requires at least one alias".into(),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }
    let outcome = crate::storage::resolve::resolve(db, id)?;
    let target = match outcome {
        crate::storage::resolve::ResolveOutcome::Hit { id, .. } => id,
        other => return err_forward(other),
    };

    let pairs: Vec<(String, String)> = aliases
        .iter()
        .map(|a| (a.clone(), target.clone()))
        .collect();
    if let Err(e) = db.add_aliases(&pairs) {
        let msg = e.to_string();
        let code = if msg.contains("id_conflict") {
            "id_conflict"
        } else if msg.contains("alias_conflict") {
            "alias_conflict"
        } else if msg.contains("not_found") {
            "not_found"
        } else if msg.contains("empty_alias")
            || msg.contains("alias_too_long")
            || msg.contains("alias_control_chars")
        {
            "invalid_alias"
        } else {
            "alias_error"
        };
        return Ok(CmdError {
            code,
            message: format!("cannot alias -> '{target}': {msg}"),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }
    for alias in aliases {
        println!("{}", serde_json::json!({ "aliased": alias, "to": target }));
    }
    Ok(0)
}

/// `pst unalias <alias>...`
pub fn cmd_unalias(db: &Database, aliases: &[String]) -> Result<i32> {
    if aliases.is_empty() {
        return Ok(CmdError {
            code: "usage",
            message: "unalias requires at least one alias".into(),
            extra: vec![],
            exit: 1,
        }
        .emit());
    }
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

/// Canonical id validation lives in the model so storage/import share it.
pub use crate::model::is_canonical_id;
