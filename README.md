# pst — Prompt STorage

<div align="center">
  <img src="pst_illustration.webp" alt="pst — your personal prompt library, one command away">
</div>

<div align="center">

![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue.svg)
![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)
![Release](https://img.shields.io/github/v/release/quangdang46/prompt_storage?include_prereleases)

</div>

**Your prompts, one command away.** `pst` is a local-first prompt library stored in SQLite with full-text search, variable templating, and a bridge that teaches AI coding agents how to use it. Type `pst code-review` and the prompt lands on stdout instantly — as natural as `cat`-ing a file.

<div align="center">

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/prompt_storage/main/install.sh?$(date +%s)" \
  | bash -s -- --easy-mode
```

</div>

---

## 🤖 Agent Quickstart (Robot Mode)

`pst` is designed agent-first: direct mode prints **raw prompt content on stdout** with zero decoration, so an AI coding agent can consume it without any flag.

```bash
# 1) Get a prompt by exact id (raw content on stdout)
pst code-review

# 2) Unique prefix resolves automatically (notice goes to stderr)
pst code

# 3) Search when you do not know the id
pst search "rust security review"

# 4) Suggest prompts for a task
pst suggest "review authentication code"

# 5) Render with variables filled
pst render code-review --LANGUAGE=Rust --CONTEXT=diff.txt

# 6) Machine-readable payload for any command
pst code-review --json

# 7) Teach your agents that pst exists (installs integration skill)
pst install
```

**Output contract** (frozen, tested):

| Case | stdout | stderr | exit |
|---|---|---|---|
| hit | raw content + `\n` | silent | 0 |
| `--json` | stable JSON payload | — | 0 |
| fuzzy hit | raw content | `resolved '<q>' -> '<id>' (fuzzy)` | 0 |
| not found | empty | `{"error":"not_found","query":"…"}` | 1 |
| ambiguous | empty | `{"error":"ambiguous","candidates":[…]}` | 1 |

⚠️ **Ambiguity never guesses**: when multiple prompts match, `pst` fails loudly with a candidate list instead of silently returning wrong content. An agent receiving the wrong prompt is worse than receiving none.

**AGENTS.md blurb** — paste into your repo's agent instructions:

```markdown
## Prompt library (pst)
A local prompt registry is available via `pst`.
When a task would benefit from a reusable prompt or workflow:
1. Search first: `pst search "<terms>"` / `pst suggest "<task>"`.
2. Prefer an existing relevant prompt over recreating one.
3. Retrieve with `pst <id>` — output is the raw prompt on stdout.
4. Use `pst render <id> --VAR=value` when variables/context are required.
5. Never guess when resolution is ambiguous — ask or refine.
Do NOT run `pst search` reflexively on every request; reach for it
when a reusable prompt plausibly exists.
```

---

## TL;DR

### The Problem

Prompts live scattered everywhere: markdown files in random folders, buried chat history, clipboard history you already lost, old notes apps. Every time you need *that* good code-review prompt, you either dig through folders or rewrite it from scratch — worse. And your AI coding agent has no idea any of your prompts exist.

### The Solution

`pst` collapses everything into **one SQLite database** with:

- **Instant retrieval** — `pst <id>` feels like `cat file`. Exact ≤30ms, search ≤50ms (p95 budgets enforced by bench).
- **Forgiving resolution** — can't remember the full id? unique prefixes resolve; aliases give short nicknames (`cr` → `code-review`); FTS/BM25 catches the rest.
- **Never guesses** — ambiguity fails with candidates, never silently picks wrong content.
- **Templating built-in** — `{{VARIABLE}}` placeholders with defaults, file vars, context injection, interactive fill.
- **Agent-native** — `pst install` writes a single integration skill teaching Claude/Codex *how to use* pst. Your prompts stay in the DB; they don't become skill files.

### Why pst?

| | What you get |
|---|---|
| ⚡ **Fast-first** | p95 budgets: exact hit ≤30ms, search ≤50ms, TUI paint ≤150ms — enforced by CI bench |
| 🎯 **Resolution that thinks** | exact → alias → prefix → FTS cascade with hard ambiguity protection |
| 🧬 **Templating** | `{{UPPER_SNAKE}}` vars, declared defaults, dynamic `CWD`/`PROJECT_NAME`, 100KB-capped file vars |
| 🔒 **Local-only** | No accounts, no tokens, zero network calls anywhere in the binary |
| 💾 **Durable** | SQLite WAL + forward-only migrations + atomic JSONL backup/restore (byte-equal roundtrip) |
| 🤖 **Agent bridge** | One integration skill (`prompt-storage`) wires Claude Code & Codex to the CLI |
| 🖥️ **TUI included** | `pst i` — ratatui picker, type-to-filter, copy/print/preview |
| 📦 **Collections** | Named prompt sets with ordered markdown export |

### Comparison

| | **pst** | Files/folders + grep | Cloud snippet managers |
|---|---|---|---|
| Retrieval speed | ≤30ms indexed FTS | fast but unranked | network-bound |
| Forgiving lookup | prefix + aliases + BM25 | exact filename only | varies |
| Ambiguity handling | fail-loudly w/ candidates | N/A — you eyeball | silent best-match |
| Variables/templating | native `{{VAR}}` + fill + context | none | rarely |
| Works offline | fully | fully | usually not |
| Agent integration | installed skill teaches agents to use it | none | none/API-key fuss |
| Backup story | atomic JSONL, byte-equal restore | manual copies | vendor lock-in |
| Privacy | everything local, single SQLite file | local | cloud-hosted |

---

## Quick Example

```bash
# Create from stdin (heredoc)
pst new code-review --title "Code Review Assistant" --category debugging --tag review <<'EOF'
Review this {{LANGUAGE}} code for bugs, performance, security, style:

{{CODE}}

Provide specific, actionable feedback.
EOF

# Give it a short alias
pst alias code-review cr

# Now retrieve — all three print identical raw content
pst code-review
pst cr
pst cod            # unique prefix

# Not sure of the id?
pst search "code review"
pst suggest "review my auth module"

# Render with variables
pst render code-review --LANGUAGE=Rust --CODE=$(cat lib.rs)

# Or inject context from a file (JSON/TOML flat key-value)
pst render code-review --context ctx.json

# Copy straight to clipboard
pst cr --copy

# Group prompts
pst collection create rust-work --desc "Rust daily drivers"
pst collection add rust-work code-review clippy-fix idiomatic-rust
pst collection export rust-work --stdout > rust-prompts.md

# Browse interactively (TUI)
pst i

# Backup everything — atomic JSONL
pst export --out backup.jsonl

# Restore after a machine swap — byte-equal roundtrip
pst import backup.jsonl --replace

# Health check (detects FTS drift, repairs with --fix)
pst doctor
```

---

## Design Principles

| Principle | In practice |
|---|---|
| **Clean stdout is sacred** | Direct mode prints raw content only — no banners, colors, metadata. Piping is always safe. |
| **Resolution never guesses** | Exact → alias → prefix → FTS; ties fail loudly with scored candidates. Wrong-but-plausible output is the worst failure mode. |
| **DB and index always agree** | Every mutation is one transaction; doctor detects drift; `doctor --fix` heals. |
| **Fast-first, boring tech** | Bundled SQLite, WAL, FTS5. No daemons, no network, no dynamic-linking surprises. |
| **CLI → Core → Storage** | CLI and TUI are thin presentation layers over one core library — behavior can never diverge between frontends. |
| **Agent-safe by construction** | Stable error payloads, non-TTY defaults, `$EDITOR` never invoked without a TTY — agents can't hang. |

---

## Installation

```bash
# macOS / Linux — install script (latest release)
curl -fsSL "https://raw.githubusercontent.com/quangdang46/prompt_storage/main/install.sh?$(date +%s)" \
  | bash -s --

# Pin a version + verify checksums
curl -fsSL "https://raw.githubusercontent.com/quangdang46/prompt_storage/main/install.sh" \
  | bash -s -- --version v0.1.0 --verify

# From source
git clone https://github.com/quangdang46/prompt_storage.git
cd prompt_storage
cargo build --release
install -m 755 target/release/pst ~/.local/bin/
```

Prebuilt binaries ship for `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64` with `SHA256SUMS.txt`.

---

## Commands

### Direct access

```bash
pst <id-or-query>        # resolution engine: exact → alias → prefix → FTS
pst get <query>          # explicit alias of direct mode
pst show <id>            # human view: metadata + preview
```

### CRUD

```bash
pst new <id> [--title T] [--category C] [--tag t]… [--from FILE|-|$EDITOR]
pst edit <id>            # $EDITOR (TTY only)
pst rm <id> [--force]
pst alias <id> <alias>…  # short nicknames, case-insensitive
pst unalias <alias>…
```

### Discovery

```bash
pst list [--category C] [--tag t] [--featured] [--limit N]
pst search <query> [--limit N]      # FTS5 + BM25 ranking
pst suggest <task>                  # recommendations with reasons
pst categories                      # counts per category
pst tags                            # counts per tag
pst random [--category C] [--copy]
```

### Execution

```bash
pst copy <id> [--fill] [--VAR=value…]
pst render <id> [--fill] [--context FILE] [--stdin] [--VAR=value…]
```

Variables: `{{UPPER_SNAKE}}` placeholders · declared defaults · dynamic `CWD` / `PROJECT_NAME` · `file` type reads file contents (100KB cap) · missing required vars → `missing_variables` error listing names · unfilled placeholders pass through untouched.

### Portability

```bash
pst export [--all | --ids …] [--format jsonl|md] [--out PATH] 
pst import <file.jsonl> [--merge|--replace]
```

JSONL format: `_meta` header line + one self-contained line per prompt (embeds variables and aliases). Restore guarantee: export → destroy DB → import → byte-identical.

### Collections

```bash
pst collections                          # list
pst collection create <name> [--desc D]
pst collection add <name> <id>…
pst collection remove <name> <id>…
pst collection export <name> [--stdout]  # ordered markdown
pst collection delete <name>
```

### Agent integration

```bash
pst install              # write .agents/skills/prompt-storage/SKILL.md
                         # + link into Claude Code / Codex skill dirs
pst uninstall            # remove ONLY that skill — never touches your DB
```

Idempotent: re-running detects unchanged content via sha256 and no-ops. Canonical copy lives in `.agents/skills/prompt-storage/`; adapters symlink it into `<root>/.claude/skills/` and `<root>/.codex/skills/`.

### System

```bash
pst config [get|set|list|reset|path]   # TOML at ~/.config/pst/config.toml
pst status                              # db path, count, schema version
pst doctor [--fix]                      # integrity, FTS drift detect + repair
pst completion --shell bash|zsh|fish|powershell
pst i                                   # TUI picker
```

---

## Configuration

`~/.config/pst/config.toml` (override the whole root with `PST_HOME`):

```toml
[output]
color = true             # ANSI color on TTY (respects NO_COLOR)
json = false             # default JSON for enumeration commands

[suggest]
default_limit = 3        # suggestions shown by pst suggest
```

Environment overrides: `PST_HOME` (relocates DB + config), `NO_COLOR` / `PST_NO_COLOR` (disable color).

Data lives at:

| Content | Path |
|---|---|
| Database | `~/.local/share/pst/store.db` (+WAL) |
| Config | `~/.config/pst/config.toml` |
| Integration skill | `<root>/.agents/skills/prompt-storage/` |

---

## Performance

Budgets are enforced, not aspirational — `scripts/bench.sh` seeds a synthetic 10k-prompt library and fails CI if p95 exceeds:

| Operation | p95 budget |
|---|---|
| `pst <exact-id>` | ≤ 30 ms |
| `pst <unique-prefix>` | ≤ 30 ms |
| `pst search <query>` | ≤ 50 ms |
| `pst render <id> --VARS…` | ≤ 40 ms |
| `pst i` first paint | ≤ 150 ms |

How: bundled SQLite (no dynamic linking), WAL mode, FTS5 covering every searchable field, lazy init so the TUI's dependencies never touch the `pst <id>` hot path, zero startup I/O beyond opening the DB.

---

## Architecture

```
CLI (clap) ─┐
TUI (ratatui) ─┤→ core lib → storage (SQLite WAL + FTS5)
agents (skill) ┘          → resolve engine → render engine
```

Single Rust crate with a hard internal boundary: `commands/*` and `tui/*` hold presentation logic only; anything touching the DB, resolving ids, or rendering templates lives in the core library both frontends share. The agent-facing surface is just the same CLI learned via one installed skill.

Storage schema: `prompts`, `aliases` (NOCASE), `prompt_tags`, `prompt_variables`, `collections`, `collection_prompts`, `prompts_fts` (FTS5), `meta`. Schema evolves via forward-only migrations tracked in `PRAGMA user_version`; databases written by newer versions are rejected with a clear error instead of corrupting.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `{"error":"not_found","query":"…"}` | No exact/alias/prefix/FTS match | Run `pst search "<partial>"` to see what exists |
| `{"error":"ambiguous","candidates":[…]}` | Multiple prompts share your prefix | Use a longer prefix or an exact id |
| `{"error":"tty_required",…}` on `new`/`edit` | `$EDITOR` paths refuse non-TTY (agent safety) | Pipe content: `pst new <id> --from -` |
| `--fill` does nothing | Non-TTY stdin | Pass explicit `--VAR=value` flags |
| Clipboard fails on Linux | No X11/Wayland tool found | Install `xclip` or `wl-copy` |
| `{"error":"schema_too_new"}` | DB written by a newer pst | Upgrade pst; migrations are forward-only |
| Doctor reports FTS drift | Rare bug or external DB tampering | `pst doctor --fix` rebuilds the index in one transaction |
| Skill link broken after moving repo | Relative symlink target changed | Re-run `pst install` (idempotent, rewrites links) |

## Limitations

- **Local-only by design** — no sync between machines. Use `export`/`import` (or commit the JSONL to a private dotfiles repo) to move libraries.
- Single-writer: SQLite WAL allows concurrent readers but serializes writers (fine for CLI usage patterns).
- Variable templating covers text substitution only — no conditionals or loops.
- TUI requires a real terminal; `pst i` errors helpfully under pipes.

## FAQ

<details>
<summary><b>Where does pst store my data?</b></summary>

One SQLite file at `~/.local/share/pst/store.db` plus its WAL. Everything — prompts, aliases, tags, collections, variables — lives there. `PST_HOME` relocates the entire tree (useful for tests or per-project libraries).
</details>

<details>
<summary><b>How is this different from just keeping prompts in files?</b></summary>

Ranked full-text search, forgiving id resolution with ambiguity protection, variable templating with defaults, atomic backup with byte-equal restore, use-count analytics, and a one-command bridge that teaches AI agents the whole workflow. Files + grep gets you none of that consistently.
</details>

<details>
<summary><b>Why does pst fail instead of picking the closest match?</b></summary>

Because its primary consumer is often an AI agent piping stdout into a workflow. A silently-wrong prompt poisons everything downstream; a loud failure costs one retry. Ambiguity returns a ranked candidate list so you (or the agent) can pick explicitly.
</details>

<details>
<summary><b>Does installing skills upload my prompts anywhere?</b></summary>

No. `pst install` writes one local instruction file (`.agents/skills/prompt-storage/SKILL.md`) that tells agents the CLI exists and how to call it. Prompts never leave the database; there is no network code in the binary at all.
</details>

<details>
<summary><b>Can I use pst across multiple machines?</b></summary>

Not natively (deliberate v1 scope). The supported path: `pst export --out prompts.jsonl`, commit that file to a private dotfiles repo, `pst import --merge` on other machines. The roundtrip is byte-equal by test contract.
</details>

<details>
<summary><b>What happens to my prompts if I run pst uninstall?</b></summary>

Nothing. Uninstall removes only the integration skill and its links — the invariant is tested: it can never delete prompts, collections, or any database rows.
</details>

---

## Development

```bash
cargo build                     # workspace build
cargo test                      # unit + contract suites (in-memory DB, temp homes)
cargo test --test contract      # black-box CLI contract tests only
scripts/smoke.sh                # end-to-end lifecycle incl. drift heal + reinstall
scripts/bench.sh                # perf budgets on seeded 10k-prompt library
```

The contract suite is the executable spec: it spawns the real binary with an isolated `PST_HOME` and asserts exact stdout/stderr/exit triples. If you change observable behavior, you change the contract tests first.

MIT — see [LICENSE](LICENSE).

<div align="center">
<i>pst — because your best prompt shouldn't live in scrollback.</i>
</div>
