---
name: prompt-storage
description: Local prompt registry available via the pst CLI. Teaches agents how to find, retrieve, and render reusable prompts instead of recreating them.
---

# Prompt Storage

`pst` is a local, DB-backed personal prompt library. It stores reusable
prompts in SQLite with full-text search and variable templating.

## When to use

When a task would benefit from a reusable prompt or workflow:
1. Search pst first (pst search / pst suggest).
2. Prefer an existing relevant prompt over recreating one.
3. Retrieve it with pst <id>; output is the raw prompt on stdout.
4. Use pst render <id> --VAR=value when variables/context are required.
5. Never guess when resolution is ambiguous - ask the user or refine the query.

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
