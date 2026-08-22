# COMPREHENSIVE_PLAN_FOR_PROMPT_STORAGE.md

**Project:** `prompt_storage` — repo at `~/Projects/prompt_storage/`
**Binary:** `pst`
**One-line pitch:** a local-first personal prompt library that is as fast and natural as `cat`-ing a file — usable by humans (CLI/TUI) and by AI coding agents (via the `prompt-storage` integration skill).

Internal design precedents distilled during planning: WAL-backed SQLite storage, FTS5 BM25 ranking, atomic JSONL mirroring, and stable machine-readable error payloads. Detailed provenance lives in project history, not this document.

---

## 1. Vision

Prompts today live scattered across markdown files, chat history, clipboards, and old notes. `pst` collapses them into a **single source of truth**: one SQLite database with full-text search, variables/templating, backup, and a bridge into coding agents.

The core experience:

```bash
pst code-review        # exact hit -> raw prompt on stdout, instantly
pst code               # unique prefix -> resolves, warns on stderr
pst search rust security
pst suggest "review authentication code"
pst i                  # TUI picker for humans exploring the library
pst render code-review --LANGUAGE=Rust --CONTEXT=diff.txt
```

Three invariants, in priority order (nothing may compromise them):

1. **Clean raw stdout** — `pst <id> | cat` is byte-exact prompt content. No banners, no color, no metadata leakage.
2. **Resolution never guesses** — ambiguity always fails loudly with candidates; a wrong-but-plausible prompt is worse than no prompt.
3. **DB and FTS always consistent** — every mutation is transactional; drift is detectable and repairable.

Design philosophy: **fast-first, boring tech, no platform**. CLI → Core → Storage. All frontends (CLI, TUI) call the same core library; none reimplement resolution/search/render.

---

## 2. Scope Matrix

### In scope (v1)

| Area | Contents |
|---|---|
| Storage | SQLite WAL, transactions everywhere, FTS5/BM25, migration framework, atomic JSONL export/import |
| Direct access | `pst <id>` resolution engine: exact → alias → prefix → FTS, ambiguity protection |
| CRUD | `new/edit/rm`, metadata (title/description/category/tags/difficulty/author/version), aliases |
| Discovery | `list`, `search`, `suggest`, `categories`, `tags`, `random` |
| Execution | `copy` (platform clipboard tools), `render` with `{{VAR}}` templating, `--fill`, `--context`, `--stdin`, file vars |
| Grouping | Collections (named sets + markdown export) |
| Portability | JSONL backup/restore (atomic, merge/replace), markdown export |
| Human UI | TUI interactive picker (`pst i`) — required core UX, fast-first |
| Agent integration | `pst install` → single `prompt-storage` skill in `.agents/skills/`, linked/copied into agent skill dirs via adapters (Claude Code, Codex) |
| System | `config` (TOML), `status`, `doctor` (incl. FTS drift detection + `--fix`), `completion` |

### Out of scope (v1, deliberate)

| Dropped | Rationale |
|---|---|
| Remote service / auth / premium / cloud sync | `pst` is local-only. No accounts, no tokens, no network calls anywhere in the binary. |
| MCP server (`serve`) | Workflow is human→`pst` and agent→shell→`pst`. An MCP layer adds JSON-RPC protocol burden with no core value. Revisit only if external clients ever need to call pst as a service. |
| Per-prompt / per-bundle SKILL.md install | Wrong mental model. Prompts live in the DB; agents learn to *use* pst through the single integration skill. Markdown export still exists for sharing. |
| `update-cli` self-update | Release infrastructure concern, not core. Can be added post-v1 without touching architecture. |
| Public `refresh` command | FTS rebuild is an internal recovery op exposed as `doctor --fix`, not a user-facing feature. |
| Bundles | Their purpose (grouped skill installs) is gone; collections cover grouping. Schema can gain them later via migration if ever needed. |
| Notes | Deferred. Cheap to add later via migration; not part of the daily loop. |
| Registry SWR/ETag loader | No remote registry exists. |

### Final v1 stamp

```text
CORE      SQLite + FTS5 + migrations · CRUD · aliases · exact/prefix/FTS resolution ·
          search/suggest · tags/categories · dynamic variables · render/fill/context/stdin ·
          copy · collections · JSONL backup/import/export
UX        TUI picker (pst i) — required core UX
AI AGENT  prompt-storage integration skill (.agents/skills/prompt-storage)
          + Claude adapter + Codex adapter
SYSTEM    config · status · doctor --fix · completion
NOT V1    MCP · update-cli · public refresh · per-prompt SKILL.md · bundles · notes ·
          about · remote/auth/cloud
```

---

## 3. Architecture

Single crate `crates/pst` with an internal library boundary so integration tests and the TUI exercise the same core the CLI uses:

```
prompt_storage/
├── Cargo.toml                     # workspace root
├── crates/pst/
│   ├── Cargo.toml                 # clap(derive), rusqlite(bundled), serde, serde_json,
│   │                              # anyhow, chrono, rand, atty, directories, toml,
│   │                              # sha2, ratatui, crossterm; dev: tempfile
│   └── src/
│       ├── lib.rs                 # exposes core::* — the ONLY entry frontends use
│       ├── main.rs                # thin: parse argv (incl. --VAR=value prescan), dispatch
│       ├── model/
│       │   └── prompt.rs          # Prompt, PromptVariable, VariableType, Collection, Alias
│       ├── storage/
│       │   ├── schema.rs          # CREATE_SCHEMA (v1) + ordered MIGRATIONS list
│       │   ├── database.rs        # Database wrapper; pragmas; tx-wrapped mutations
│       │   ├── jsonl.rs           # atomic export/import (temp->fsync->rename)
│       │   └── resolve.rs         # friendly-id resolution engine
│       ├── render/
│       │   └── mod.rs             # {{VAR}} substitution, defaults, file/path vars, caps
│       ├── clipboard.rs           # pbcopy/xclip/xsel/clip detection + invocation
│       ├── skills/
│       │   ├── skill_md.rs        # generate prompt-storage SKILL.md content
│       │   └── agents.rs          # AgentAdapter trait + claude/codex impls
│       ├── tui/
│       │   └── mod.rs             # ratatui picker (thin consumer of core)
│       └── commands/              # one module per subcommand; each calls core only
│           ├── mod.rs  direct.rs  show.rs  new.rs  edit.rs  remove.rs
│           ├── alias.rs  list.rs  search.rs  suggest.rs  taxonomy.rs  random.rs
│           ├── copy.rs  render.rs  export.rs  import.rs  collections.rs
│           ├── install.rs  uninstall.rs  config.rs  status.rs  doctor.rs
│           ├── completion.rs  tui_entry.rs
├── scripts/smoke.sh               # end-to-end black-box script
└── README.md
```

**Rule:** `commands/*` and `tui/*` contain presentation logic only. Any function that touches the DB, resolves an id, or renders a template lives under `core` (`model`/`storage`/`render`) and is reachable through `lib.rs`.

### Data paths (XDG; `PST_HOME` overrides everything)

| Content | Path |
|---|---|
| SQLite store | `~/.local/share/pst/store.db` (+ `-wal`, `-shm`) |
| Config | `~/.config/pst/config.toml` |
| Integration skill (canonical) | `<repo-or-home>/.agents/skills/prompt-storage/SKILL.md` |
| Agent-specific links | adapter-defined (see §11) |

---

## 4. Storage Layer

### Migration framework (day one — not an afterthought)

```rust
pub const LATEST_SCHEMA_VERSION: i32 = 1;

/// Ordered, append-only. Entry i upgrades version i-1 -> i.
pub const MIGRATIONS: &[&str] = &[
    // 0 -> 1
    r#"CREATE TABLE IF NOT EXISTS prompts (...); ..."#,
];

fn run_migrations(conn: &Connection) -> Result<()> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i32;
        if current < target {
            conn.execute_batch(&format!("BEGIN; {sql}; PRAGMA user_version = {target}; COMMIT;"))?;
        }
    }
    Ok(())
}
```

Rules: forward-only migrations; each runs inside one transaction; `PRAGMA user_version` is the source of truth (not a meta row); runtime rejects databases with `user_version > LATEST` (downgrade guard). Data-version markers (`data_version`, `last_sync`-style timestamps) live in `meta` and are separate from schema versioning.

### Schema v1

```sql
CREATE TABLE IF NOT EXISTS prompts (
    id            TEXT PRIMARY KEY,              -- kebab-case canonical
    title         TEXT NOT NULL,
    content       TEXT NOT NULL,
    description   TEXT,
    category      TEXT,
    tags_text     TEXT,                          -- denormalized for FTS
    version       TEXT,
    author        TEXT,
    difficulty    TEXT,                          -- beginner|intermediate|advanced|NULL
    featured      INTEGER NOT NULL DEFAULT 0,
    source        TEXT NOT NULL DEFAULT 'manual',-- manual|imported
    use_count     INTEGER NOT NULL DEFAULT 0,    -- tie-break signal for resolution
    last_used_at  TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS aliases (
    alias     TEXT PRIMARY KEY COLLATE NOCASE,   -- case-insensitive uniqueness
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS prompt_tags (
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    tag       TEXT NOT NULL,
    PRIMARY KEY (prompt_id, tag)
);

CREATE TABLE IF NOT EXISTS prompt_variables (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,                          -- UPPER_SNAKE_CASE
    var_type TEXT NOT NULL DEFAULT 'text',       -- text|multiline|select|file|path
    required INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    default_value TEXT
);

CREATE TABLE IF NOT EXISTS collections (
    name TEXT PRIMARY KEY COLLATE NOCASE,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS collection_prompts (
    collection_name TEXT NOT NULL REFERENCES collections(name) ON DELETE CASCADE,
    prompt_id TEXT NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (collection_name, prompt_id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    id, title, description, content, tags_text
);

CREATE INDEX IF NOT EXISTS idx_prompts_category ON prompts(category);
CREATE INDEX IF NOT EXISTS idx_prompt_tags_tag  ON prompt_tags(tag);

CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

### Operational rules

1. **Open:** create parent dirs; `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5s`, `foreign_keys=ON`; run migrations.
2. **Every mutation is one transaction** — including single-prompt upsert (tx-wrapping only bulk writes is a classic bug; we avoid it). Upsert = `INSERT … ON CONFLICT(id) DO UPDATE` + delete/reinsert tags + variables + FTS row, atomically.
3. **Search:** FTS5 MATCH with BM25 weights `id=5, title=3, description=2, content=1, tags=2`; negate score for ranking (BM25 returns negative values); escape query text before MATCH (`escape_fts_query`).
4. **JSONL:** first line `{"_meta":{version,count,exported_at,schema_version}}`; one prompt per line (embeds its variables and aliases); write to `<path>.tmp`, fsync, rename; on Windows remove destination first. Import modes: `--merge` (upsert by id) and `--replace` (wipe + insert in one tx); skip `_meta` lines; reject files whose `schema_version > LATEST`.
5. **Testing:** `Database::in_memory()` for all unit tests — zero disk contact.

---

## 5. Friendly-ID Resolution Engine

Used identically by `pst <query>` and every command taking `<id>`.

```
1. EXACT   : prompts.id == query                        -> hit
2. ALIAS   : aliases.alias == query (COLLATE NOCASE)    -> hit
3. PREFIX  : ids ∪ aliases starting with query
             ├─ exactly 1      -> hit
             └─ more than 1    -> AMBIGUOUS (candidates sorted: use_count DESC, then alpha)
4. FUZZY   : FTS BM25 top-k (k = 8)
             ├─ top1 beats top2 by >= 40% AND passes min-score floor -> hit (notice on stderr)
             ├─ leading but not decisively                           -> AMBIGUOUS (scored candidates)
             └─ no results                                           -> NOT_FOUND
```

On hits from steps 3–4: bump `use_count`, set `last_used_at` (one tx). Prefix tie-breaks therefore favor frequently used prompts over time.

### Ambiguity policy (hard rule)

Never pick when unclear. Print candidate table (`id — title — score/source`) to stderr plus machine-readable `{"error":"ambiguous","candidates":[…]}`; exit 1. An agent receiving wrong content silently is the worst possible failure mode.

### Naming invariants

- Canonical id: `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
- Alias: `^[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}$`.
- **Collision invariants (enforced inside the insert transaction):**
  - alias must not equal any canonical id — exact (`foo`) *and* case-insensitively (`FOO` vs id `foo`) since lookup is NOCASE;
  - alias must be unique among aliases case-insensitively;
  - violations return `alias_conflict` / `id_conflict`.
  - Concurrency: WAL allows one writer; the check-and-insert pair inside one transaction is race-free.
- Collection names follow the same safe-id regex as aliases.

---

## 6. Output Contract (this IS the API for AI agents)

| Invocation | stdout | stderr | exit |
|---|---|---|---|
| `pst <id>` (TTY or piped) | raw content + `\n` | silent | 0 |
| `pst <id> --json` | `{id,title,content,description?,category?,tags[],variables?,version?,use_count,last_used_at}` | — | 0 |
| `pst <id> --copy` | (TTY: short preview; piped: nothing) | `Copied: <title>` | 0 |
| fuzzy-hit notice | raw content | one-line `resolved '<q>' -> '<id>' (fuzzy)` | 0 |
| not found | empty | `{"error":"not_found","query":"…"}` | 1 |
| ambiguous | empty | `{"error":"ambiguous","candidates":[{id,title,score}…]}` | 1 |
| db error | empty | `{"error":"database_error","message":…}` | 1 |

Principles:
- **Direct mode defaults to raw text.** Unlike designs that auto-switch to JSON on non-TTY stdout (which would corrupt piped content), only enumeration commands (`list/search/suggest/categories/tags/status/doctor`) auto-switch to JSON when stdout is not a TTY.
- Error payloads are minimal, stable shapes.
- Global flags: `--json/-j`, `--no-color`; respect `NO_COLOR` / `PST_NO_COLOR` env vars.

---

## 7. Variables & Rendering

Behavioral spec:

- Placeholder syntax `{{VAR_NAME}}`; extraction/substitution regex `\{\{\s*([a-zA-Z0-9_]+)\s*\}\}`; case-sensitive exact-name matching; unfilled placeholders pass through untouched.
- Variable declarations stored per prompt (`prompt_variables`): type `text|multiline|select|file|path`, `required`, `default_value`, `description`.
- Defaults: declared `default_value` applies when no explicit value; dynamic defaults `CWD` and `PROJECT_NAME` derive from process cwd.
- Type behaviors: `file` reads file content into the value; `path` passes the path string; file reads capped at 102400 bytes with suffix notice `[File truncated to 102400 bytes from <N> bytes]`.
- `--fill`: interactively prompts missing variables; Ctrl+C → `{"error":"cancelled"}`, exit 130. Non-TTY + `--fill` → error directing to explicit `--VAR=` flags.
- Missing required vars without `--fill` → `missing_variables` error listing names.
- Context injection: `--context FILE` (JSON or TOML flat key/value) and `--stdin` (wins if both) supply additional substitutions; `--max-context` byte cap (default 204800).

### `--VAR=value` argv design (the sharp edge — specified up front)

clap cannot declare arbitrary flags, so `main.rs` **pre-scans argv** before clap:

- Token grammar: `^--[A-Za-z_][A-Za-z0-9_]*=(.*)$` — split on the **first** `=` only.
- Matching tokens are extracted into a `Vec<(String,String)>` and removed from argv; clap then parses the remainder normally.
- **Reserved-flag precedence:** tokens whose name matches a real flag (`--json`, `--no-color`, …) are never treated as variables; `--json=x` is a hard error.
- Edge cases covered by tests: `--FOO=` (empty value OK), `--FOO=a=b` (value keeps second `=`), `--foo=bar` (accepted; substitution still case-sensitive so it will not match `{{FOO}}` — warn on stderr when a provided name differs only by case from a declared variable), `--A_B=123`, quoted values arrive pre-unquoted from the shell.
- Provided names matching nothing declared and appearing nowhere in content → warning on stderr (typo catcher), never fatal.

---

## 8. CLI Command Surface (final)

```
# Direct access
pst <id-or-query> [--json] [--copy]     direct mode (resolution engine)
pst get <id-or-query>                    explicit alias of direct mode
pst show <id>                            human view: metadata + preview (never piped-content path)

# CRUD
pst new <id> [--title T] [--desc D] [--category C] [--tag t]… [--difficulty D]
        [--author A] [--from FILE|-|$EDITOR] [--force]
pst edit <id>                            $EDITOR on content (+ --meta for fields)
pst rm <id> [--force]
pst alias <id> <alias>…
pst unalias <alias>…

# Discovery
pst list|ls [--category C] [--tag t] [--featured] [--limit N]
pst search <query> [--limit N]
pst suggest <task> [--limit N]           FTS + reason strings
pst categories
pst tags
pst random [--category C] [--tag t] [--copy]

# Execution
pst copy <id> [--fill] [--VAR=value…]
pst render <id> [--fill] [--context FILE] [--stdin] [--max-context B] [--VAR=value…]

# Portability
pst export [--ids …|--all] [--format jsonl|md] [--out PATH|DIR] [--stdout]
pst import <file.jsonl> [--merge|--replace]

# Grouping
pst collections
pst collection <name>                    detail: member list
pst collection create <name> [--desc D]
pst collection add <name> <prompt-id>…
pst collection remove <name> <prompt-id>…
pst collection delete <name>
pst collection export <name> [--format md] [--out PATH] [--stdout]

# Agent integration
pst install                              write prompt-storage skill + wire agent adapters
pst uninstall                            remove ONLY the prompt-storage skill + its links (never touches DB)
pst doctor                               see §12

# System
pst config [get|set|list|reset|path] [key] [value]
pst status
pst completion --shell bash|zsh|fish|powershell
pst i                                    TUI picker (§10)
```

Naming decisions locked: no separate `get.rs` — `pst get` maps to direct mode; `show` is the human-oriented metadata view; `new` is primary (add is its alias).

---

## 9. JSONL Format

```jsonc
// line 1
{"_meta":{"version":"2026-08-22T…","count":42,"exported_at":"…","schema_version":1}}
// subsequent lines
{"id":"code-review","title":"Code Review Assistant","content":"…",
 "description":"…","category":"debugging","tags":["review","quality"],
 "variables":[{"name":"CODE","type":"multiline","required":true,"default":null}],
 "aliases":["cr","review-code"]}
```

Round-trip guarantee: export → destroy DB → import → every prompt byte-identical, aliases and variables restored.

---

## 10. TUI (`pst i`) — Required Core UX

Fast-first picker, not a heavyweight app:

```
┌─────────────────────────────────────────────┐
│ Search: code review_                         │
├─────────────────────────────────────────────┤
│ > code-review                                │
│   security-review                            │
│   refactor-review                            │
├─────────────────────────────────────────────┤
│ Enter: use   Ctrl-C: exit   ↑↓: navigate    │
└─────────────────────────────────────────────┘
```

Requirements:
- Built with `ratatui` + `crossterm` (already workspace deps); incremental filter as you type using the same core search as `pst search`.
- Enter on selection → action menu: copy / print to stdout after exit / open preview pane. Exit restores terminal state cleanly (alternate screen teardown on panic via panic hook).
- Budget: first paint ≤ 150 ms on a few-thousand-prompt library; keystroke-to-filter latency imperceptible (< 16 ms per frame budget).
- Non-TTY invocation of `pst i` → error suggesting `pst search`.
- The TUI consumes `core::storage` + `core::resolve` only — zero duplicated logic.

---

## 11. Agent Integration: the `prompt-storage` Skill

Two distinct concepts, never conflated:

1. **Integration skill** (`prompt-storage`) — teaches agents that pst exists and how to use it. Installed once.
2. **Prompts in the DB** — reusable knowledge; they stay in SQLite, never become individual SKILL.md files.

### `pst install`

- Generates `SKILL.md` for the `prompt-storage` skill containing:
  - what pst is (local prompt registry; DB-backed);
  - command cheatsheet: `pst <id>`, `pst search`, `pst suggest`, `pst render --VAR=…`;
  - behavioral rules for the agent:
    ```text
    When a task would benefit from a reusable prompt or workflow:
    1. Search pst first (pst search / pst suggest).
    2. Prefer an existing relevant prompt over recreating one.
    3. Retrieve it with `pst <id>`; output is the raw prompt on stdout.
    4. Use `pst render <id> --VAR=value` when variables/context are required.
    5. Never guess when resolution is ambiguous — ask the user or refine the query.
    ```
  - note that `PST_HOME` scoping applies.
  - **Conditional reuse, not reflexive search:** the rules above are gated on "would benefit". Agents must NOT run `pst search` on every request — weigh latency and token cost; reach for pst when a reusable prompt/workflow plausibly exists.
- Writes canonical copy to `<root>/.agents/skills/prompt-storage/SKILL.md` where root = project dir when run inside a project (detected via `.git` presence heuristic + `--project/--personal` override), else `$HOME`.
- **Idempotent:** re-running with unchanged generated content is a no-op (content sha256 compared); changed template → regenerates; `--force` always rewrites.
- **Agent adapters** (trait `AgentAdapter`, registered list — nothing hardcoded into core):

  | Adapter | Skill location | Wiring |
  |---|---|---|
  | `claude` | `<root>/.claude/skills/prompt-storage` | symlink → `../../.agents/skills/prompt-storage` (copy fallback on filesystems without symlink support) |
  | `codex` | `<root>/.codex/skills/prompt-storage` | symlink, same fallback |

  Adapter responsibilities: report whether its environment is present, provide paths, create/verify links. Detection failures are reported, never fatal — canonical copy always lands in `.agents/skills/`.

### `pst uninstall`

**Invariant:** uninstall removes *only* the `prompt-storage` integration skill — the canonical `.agents/skills/prompt-storage/` directory plus adapter links pst itself created. Ownership verification needs no manifest file: a link is pst's iff it resolves to the canonical dir; a copied file is pst's iff it byte-matches regenerated content. Foreign files are never touched, and **no prompts, collections, or database rows are ever deleted by uninstall.**

---

## 12. `doctor` — Health Checks & Repair

Checks (each returns ok/warn/fail + message, JSON-friendly):

```
✓ database opens, integrity_check passes
✓ schema_version == LATEST (warn if older: "run again to migrate" — auto-migrates on open anyway)
✓ FTS index consistency (see below)
✓ editor available ($EDITOR/$VISUAL set)
✓ clipboard tool present (pbcopy|xclip|xsel|clip)
✓ prompt-storage skill present at .agents/skills/
✓ adapter links valid (symlink resolves; content hash matches canonical)
```

**FTS drift detection:** `SELECT id FROM prompts EXCEPT SELECT id FROM prompts_fts` and the inverse — any difference = fail with counts.

`--fix` performs repair in one transaction: wipe and rebuild `prompts_fts` from `prompts`, then re-run the consistency check. (Rebuild is a recovery operation, not a feature.)

---

## 13. Performance Budgets

Measured end-to-end (process spawn included), p95, on a synthetic 10k-prompt library (generated in smoke fixtures):

| Operation | Budget |
|---|---|
| `pst <exact-id>` | ≤ 30 ms |
| `pst <unique-prefix>` | ≤ 30 ms |
| `pst search <query>` | ≤ 50 ms |
| `pst render <id> --VARS…` | ≤ 40 ms |
| `pst i` first paint | ≤ 150 ms |

Enablers: WAL + prepared statements reused per-process lifetime is irrelevant for one-shot CLI (process exits), so the wins are: bundled SQLite with warm page cache from OS, FTS covering all searchable fields, no network, no dynamic linking surprises (static rusqlite), lean startup (lazy-init only what the command needs — e.g., TUI deps never touched by `pst <id>`).

Regression guard: a bench harness (`scripts/bench.sh`) seeds 10k prompts and asserts budgets in CI-optional mode; fails loudly locally.

---

## 14. Phases

### Phase 0 — CLI Contract Tests (executable spec, written first)

Freeze behaviors as black-box tests spawning the built binary (`PST_HOME=$(mktemp -d)`):

```
pst foo            -> stdout: content+\n, stderr: "", exit 0
pst foo --json     -> stable payload shape
pst foo --copy     -> stderr "Copied:", exit 0
pst unknown        -> stdout "", stderr {"error":"not_found",...}, exit 1
pst amb            -> stderr {"error":"ambiguous","candidates":[...]}, exit 1
pst foo | cat      -> byte-equal content
pst list           (non-TTY) -> JSON array payload
pst search x       -> ranked JSON
```

These start red and drive implementation. The contract — not the code — is the product for agents.

**Accept:** test suite compiles and runs (failing) against a stub binary; CI wiring proves it executes the real binary once Phase 2 lands.

### Phase 1 — Storage Core

Crate scaffold, `model/`, `storage/{schema,database,jsonl}.rs`, migration runner, in-memory test DB. Unit tests: roundtrip upsert/get, filters, FTS behavior, query escaping, JSONL roundtrip incl. variables+aliases, downgrade guard, migration idempotence.

**Accept:** `cargo test` green; sqlite3 inspection of a real store.db shows expected tables; opening twice is stable.

### Phase 2 — Resolution Engine + Core Commands

`storage/resolve.rs` (exact/alias/prefix/fuzzy + use_count tracking + ambiguity payloads); commands: direct/get/show/new/edit/rm/list/search/suggest/categories/tags/random/alias/unalias; global flags; argv prescan for `--VAR=value`.

**Accept:** all Phase 0 contract tests green; exhaustive resolve-decision unit matrix passes; `--FOO=a=b` style edge cases pass.

### Phase 3 — Render + Clipboard + Portability

Full render engine (defaults, file/path vars, caps, context/stdin, fill, cancelled-exit-130), clipboard via platform tools, `export`/`import` (merge/replace), markdown export with safe-filename + traversal guards.

**Accept:** `pst render code-review --CODE=f.rs` substitutes correctly; truncation notice appears for >100KB file vars; JSONL wipe-restore roundtrip byte-equal.

### Phase 4 — Collections + System Commands

Collections CRUD + markdown export; `config` TOML (atomic writes, typed validation); `status`; `completion` (4 shells); `doctor` incl. FTS drift detection and `--fix` rebuild.

**Accept:** doctor detects artificially corrupted FTS (test injects divergence) and `--fix` heals it; collection export produces correct markdown.

### Phase 5 — TUI

Ratatui picker per §10, consuming core only.

**Accept:** manual session: open < 150 ms, type-to-filter live, Enter copies, Ctrl-C exits restoring terminal; scripted PTY test covers open/filter/select/exit.

### Phase 6 — Integration Skill + Adapters

`skills/{skill_md,agents}.rs`; `pst install`/`uninstall`; idempotency via sha256; claude/codex adapters; doctor gains skill/link checks.

**Accept:** `pst install && pst install` → single skill dir; tampering with the link is detected by doctor; skill content contains the five behavioral rules verbatim.

### Phase 7A — Release Engineering (v1 finale)

README + illustration, `scripts/build-releases.sh`, GitHub Actions matrix (darwin/linux × arm64/x64), SHA256SUMS.txt, curl-pipe installer (bash; ps1 optional follow-up).

**Accept:** installer works locally on macOS arm64; released binaries pass their own smoke.sh in CI.

*(Self-update deferred beyond v1 — tracked as future work, non-blocking.)*

---

## 15. Testing Strategy

1. **Contract tests (Phase 0)** — black-box, spawn binary, assert stdout/stderr/exit triples. Highest-value suite; protects the agent-facing API.
2. **Unit (core)** — storage on in-memory DB with side-effect assertions (FTS rows, tag rows); resolve decision matrix; render edge cases; migration transitions.
3. **Golden JSON** — freeze payload shapes for `--json` outputs and errors; any change is a deliberate, reviewed contract bump.
4. **Integration (skills)** — temp HOME, install → assert files + hashes; modify → guard triggers; uninstall cleans owned artifacts only.
5. **PTY test for TUI** — scripted keystrokes, assert screen states and clean teardown.
6. **E2E smoke (`scripts/smoke.sh`)** — fresh PST_HOME: new → get → alias → prefix → fuzzy → render → install → export → wipe → import → get byte-equal; also exercises doctor --fix healing injected FTS drift.
7. **Perf bench** — seeded 10k library asserting §13 budgets.
8. No network mocking — there is no network code.

---

## 16. Risks & Locked Decisions

| Risk | Locked decision |
|---|---|
| Agent silently receives wrong prompt | Ambiguity always fails with candidates; never auto-select ties |
| FTS MATCH panics on special chars | Escape before every MATCH |
| BM25 negative scores invert ranking | Negate at display/rank time |
| Half-applied mutation desyncs FTS | Single transaction per mutation; doctor detects residual drift; `--fix` heals |
| Windows rename-over-existing fails | Remove destination before rename in jsonl writer |
| Dynamic `--VAR=value` eaten by clap | Pre-scan argv before clap; reserved flags win; edge-case test battery |
| Content backticks break markdown fences | Dynamic fence length on export |
| Path traversal via ids/filenames in export | Strict regexes + resolved-path containment checks; unsafe id → exit 2 |
| `$EDITOR` hangs agents | Edit/new-editor paths require TTY; non-TTY gets actionable error pointing at `--from -` |
| Case confusion between alias/id | NOCASE collation + collision invariants enforced in-tx |
| Schema evolution bricks user DBs | Forward-only migration runner from day one; downgrade guard |
| TUI becomes architectural tar pit | TUI is a thin core consumer; core ships and is fully testable before TUI exists |
