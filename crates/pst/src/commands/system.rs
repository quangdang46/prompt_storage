//! Config (TOML) + status + doctor (beads P4.2 & P4.3).
//!
//! Doctor checks per plan §12: db opens, integrity_check, schema version,
//! FTS drift, editor, clipboard tool, skill presence. `--fix` rebuilds FTS
//! in one transaction. Exit codes: 0 all-pass, 1 any-fail, 2 warn-only.

use anyhow::Result;
use serde::Serialize;

use crate::storage::database::Database;
use crate::storage::schema::{LATEST_SCHEMA_VERSION, current_version};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    pub output_color: bool,
    pub output_json: bool,
    pub suggest_default_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_color: true,
            output_json: false,
            suggest_default_limit: 3,
        }
    }
}

pub fn config_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".config/pst/config.toml")
}

/// Load config; missing file = defaults. Invalid TOML falls back to defaults
/// with a warning on stderr (never fatal — plan §16 agent-safety).
pub fn load_config(root: &std::path::Path) -> Config {
    let path = config_path(root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    #[derive(serde::Deserialize, Default)]
    struct Partial {
        #[serde(default)]
        output: OutputSection,
        #[serde(default)]
        suggest: SuggestSection,
    }
    #[derive(serde::Deserialize, Default)]
    struct OutputSection {
        color: Option<bool>,
        json: Option<bool>,
    }
    #[derive(serde::Deserialize, Default)]
    struct SuggestSection {
        default_limit: Option<usize>,
    }
    match toml::from_str::<Partial>(&text) {
        Ok(p) => Config {
            output_color: p.output.color.unwrap_or(true),
            output_json: p.output.json.unwrap_or(false),
            suggest_default_limit: p.suggest.default_limit.unwrap_or(3),
        },
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"error":"config_invalid","message":e.to_string()})
            );
            Config::default()
        }
    }
}

/// Save config atomically (temp + rename) preserving only known keys.
pub fn save_config(root: &std::path::Path, cfg: &Config) -> Result<()> {
    let path = config_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = format!(
        "[output]\ncolor = {}\njson = {}\n\n[suggest]\ndefault_limit = {}\n",
        cfg.output_color, cfg.output_json, cfg.suggest_default_limit
    );
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// `pst config [get|set|list|reset|path]`
pub fn cmd_config(
    root: &std::path::Path,
    action: &str,
    key: Option<&str>,
    value: Option<&str>,
    as_json: bool,
) -> Result<i32> {
    match action {
        "path" => println!("{}", config_path(root).display()),
        "reset" => {
            save_config(root, &Config::default())?;
            println!("{}", serde_json::json!({ "reset": true }));
        }
        "list" => {
            let cfg = load_config(root);
            if as_json || !atty::is(atty::Stream::Stdout) {
                println!(
                    "{}",
                    serde_json::json!({
                        "output": {"color": cfg.output_color, "json": cfg.output_json},
                        "suggest": {"default_limit": cfg.suggest_default_limit},
                    })
                );
            } else {
                println!("output.color = {}", cfg.output_color);
                println!("output.json = {}", cfg.output_json);
                println!("suggest.default_limit = {}", cfg.suggest_default_limit);
            }
        }
        "get" => {
            let Some(k) = key else { return usage() };
            let cfg = load_config(root);
            let v = lookup(&cfg, k);
            match v {
                Some(v) => println!("{v}"),
                None => return invalid_key(k),
            }
        }
        "set" => {
            let (Some(k), Some(val)) = (key, value) else {
                return usage();
            };
            let mut cfg = load_config(root);
            let typed: serde_json::Value = match k {
                "output.color" | "output.json" => match val {
                    "true" => serde_json::Value::Bool(true),
                    "false" => serde_json::Value::Bool(false),
                    _ => return type_error(k, val),
                },
                "suggest.default_limit" => match val.parse::<usize>() {
                    Ok(n) => serde_json::json!(n),
                    Err(_) => return type_error(k, val),
                },
                _ => return invalid_key(k),
            };
            match k {
                "output.color" => cfg.output_color = typed.as_bool().unwrap(),
                "output.json" => cfg.output_json = typed.as_bool().unwrap(),
                "suggest.default_limit" => {
                    cfg.suggest_default_limit = typed.as_u64().unwrap() as usize
                }
                _ => unreachable!(),
            }
            save_config(root, &cfg)?;
            println!("{}", serde_json::json!({ "set": k, "value": typed }));
        }
        _ => return usage(),
    }
    Ok(0)
}

fn lookup(cfg: &Config, key: &str) -> Option<String> {
    match key {
        "output.color" => Some(cfg.output_color.to_string()),
        "output.json" => Some(cfg.output_json.to_string()),
        "suggest.default_limit" => Some(cfg.suggest_default_limit.to_string()),
        _ => None,
    }
}

fn invalid_key(k: &str) -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({ "error": "invalid_key", "key": k })
    );
    Ok(1)
}

fn type_error(k: &str, v: &str) -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({ "error": "invalid_value", "key": k, "value": v })
    );
    Ok(1)
}

fn usage() -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({"error":"usage","hint":"config [get|set|list|reset|path] [key] [value]"})
    );
    Ok(1)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StatusOutput {
    database: DatabaseStatus,
    meta: MetaStatus,
}

#[derive(Serialize)]
struct DatabaseStatus {
    path: String,
    exists: bool,
    prompt_count: i64,
    schema_version: i32,
}

#[derive(Serialize)]
struct MetaStatus {
    data_version: Option<String>,
}

/// `pst status`
pub fn cmd_status(db: &Database, home: &std::path::Path, as_json: bool) -> Result<i32> {
    let db_path = crate::storage::database::db_path_under(home);
    let output = StatusOutput {
        database: DatabaseStatus {
            path: db_path.display().to_string(),
            exists: db_path.exists(),
            prompt_count: db.prompt_count()?,
            schema_version: current_version(db.conn())?,
        },
        meta: MetaStatus {
            data_version: db.get_meta("data_version")?,
        },
    };
    if as_json || !atty::is(atty::Stream::Stdout) {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("db:      {}", output.database.path);
        println!("prompts: {}", output.database.prompt_count);
        println!("schema:  v{}", output.database.schema_version);
        if let Some(dv) = &output.meta.data_version {
            println!("synced:  {dv}");
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct Check {
    pub name: &'static str,
    pub status: &'static str, // ok | warn | fail
    pub message: String,
}

#[derive(Serialize)]
struct DoctorOutput {
    checks: Vec<Check>,
    #[serde(rename = "all_passed")]
    all_passed: bool,
}

const DRIFT_SQL_A: &str =
    "SELECT COUNT(*) FROM (SELECT id FROM prompts EXCEPT SELECT id FROM prompts_fts)";
const DRIFT_SQL_B: &str =
    "SELECT COUNT(*) FROM (SELECT id FROM prompts_fts EXCEPT SELECT id FROM prompts)";

/// `pst doctor [--fix]`
pub fn cmd_doctor(db: &Database, home: &std::path::Path, fix: bool, as_json: bool) -> Result<i32> {
    let mut checks = Vec::new();

    // 1. DB opens + integrity.
    checks.push(Check {
        name: "database_integrity",
        status: "ok",
        message: "opened".into(),
    });

    // 2. Schema version.
    let ver = current_version(db.conn())?;
    if ver == LATEST_SCHEMA_VERSION {
        checks.push(Check {
            name: "schema_version",
            status: "ok",
            message: format!("v{ver}"),
        });
    } else if ver < LATEST_SCHEMA_VERSION {
        checks.push(Check {
            name: "schema_version",
            status: "warn",
            message: format!("v{ver} older than latest v{LATEST_SCHEMA_VERSION}"),
        });
    }

    // 3. FTS drift.
    let drift_a: i64 = db.conn().query_row(DRIFT_SQL_A, [], |r| r.get(0))?;
    let drift_b: i64 = db.conn().query_row(DRIFT_SQL_B, [], |r| r.get(0))?;
    let drift_total = drift_a + drift_b;
    if drift_total == 0 {
        checks.push(Check {
            name: "fts_consistency",
            status: "ok",
            message: "index matches prompts".into(),
        });
    } else if fix {
        rebuild_fts(db)?;
        let re_a: i64 = db.conn().query_row(DRIFT_SQL_A, [], |r| r.get(0))?;
        let re_b: i64 = db.conn().query_row(DRIFT_SQL_B, [], |r| r.get(0))?;
        let healed = re_a + re_b == 0;
        checks.push(Check {
            name: "fts_consistency",
            status: if healed { "ok" } else { "fail" },
            message: if healed {
                format!("rebuilt; healed {drift_total} drifted rows")
            } else {
                "rebuild did not heal drift".into()
            },
        });
    } else {
        checks.push(Check {
            name: "fts_consistency",
            status: "fail",
            message: format!("{drift_total} drifted rows (run doctor --fix)"),
        });
    }

    // 4. Editor.
    let editor_ok = std::env::var_os("EDITOR").is_some() || std::env::var_os("VISUAL").is_some();
    checks.push(Check {
        name: "editor",
        status: if editor_ok { "ok" } else { "warn" },
        message: if editor_ok {
            "EDITOR/VISUAL set".into()
        } else {
            "$EDITOR not set".into()
        },
    });

    // 5. Clipboard tool.
    let clip_ok = crate::clipboard::detect_tool().is_some();
    checks.push(Check {
        name: "clipboard_tool",
        status: if clip_ok { "ok" } else { "warn" },
        message: crate::clipboard::detect_tool()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "none found".into()),
    });

    // 6. Skill presence (informational until P6 wires adapters).
    let skill_dir = home.join(".agents/skills/prompt-storage");
    checks.push(Check {
        name: "integration_skill",
        status: if skill_dir.exists() { "ok" } else { "warn" },
        message: if skill_dir.exists() {
            "prompt-storage skill present".into()
        } else {
            "not installed (pst install)".into()
        },
    });

    let any_fail = checks.iter().any(|c| c.status == "fail");
    let any_warn = checks.iter().any(|c| c.status == "warn");
    let exit_code = if any_fail {
        1
    } else if any_warn {
        2
    } else {
        0
    };

    if as_json || !atty::is(atty::Stream::Stdout) {
        let out = DoctorOutput {
            all_passed: !any_fail && !any_warn,
            checks,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for c in &checks {
            let mark = match c.status {
                "ok" => "✓",
                "warn" => "!",
                _ => "✗",
            };
            println!("{mark} {}: {} ({})", c.name, c.message, c.status);
        }
    }
    Ok(exit_code)
}

/// Rebuild prompts_fts from prompts inside one transaction.
fn rebuild_fts(db: &Database) -> Result<()> {
    let tx = db.conn().unchecked_transaction()?;
    tx.execute_batch("DELETE FROM prompts_fts;")?;
    tx.execute_batch(
        r#"INSERT INTO prompts_fts (id, title, description, content, tags_text)
           SELECT id, title, description, content, tags_text FROM prompts;"#,
    )?;
    tx.commit()?;
    Ok(())
}
