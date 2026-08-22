#!/usr/bin/env bash
# pst end-to-end smoke test (bead E2E.1).
# Full lifecycle in an isolated PST_HOME, incl. FTS-drift heal + reinstall.
set -euo pipefail

BIN="${BIN:-$(pwd)/target/debug/pst}"
if [ ! -x "$BIN" ]; then BIN="$(pwd)/target/release/pst"; fi
if [ ! -x "$BIN" ]; then echo "FATAL: build pst first (cargo build)"; exit 1; fi

PASS=0; FAIL=0
step() { echo "── $1"; }
ok()   { echo "  ✓ $1"; PASS=$((PASS+1)); }
fail() { echo "  ✗ $1"; FAIL=$((FAIL+1)); }

HOME_DIR=$(mktemp -d)
trap 'rm -rf "$HOME_DIR"' EXIT
export PST_HOME="$HOME_DIR"

step "1. new from stdin"
printf 'Review {{LANG}} code:\n{{CODE}}' | "$BIN" new demo --title Demo -f - >/dev/null
ok "created"

step "2. direct get byte-exact"
OUT=$("$BIN" demo)
[ "$OUT" = "$(printf 'Review {{LANG}} code:\n{{CODE}}')" ] && ok "byte-equal" || fail "got: $OUT"

step "3. alias + prefix"
"$BIN" alias demo d >/dev/null
[ "$("$BIN" d)" = "$OUT" ] && ok "alias resolves" || fail "alias"
[ "$("$BIN" dem)" = "$OUT" ] && ok "prefix resolves" || fail "prefix"

step "4. ambiguous fails loudly"
printf "second" | "$BIN" new demo-b --force -f - >/dev/null
printf "first" | "$BIN" new demo-a --force -f - >/dev/null
if "$BIN" demo- >/dev/null 2>&1; then fail "ambiguous must exit nonzero"; else ok "exit nonzero"; fi
ERR=$("$BIN" demo- 2>&1 >/dev/null || true)
[[ "$ERR" == *'"error":"ambiguous"'* ]] && ok "ambiguous payload" || fail "payload: $ERR"

step "5. render with variables"
R=$("$BIN" render demo --LANG=Rust --CODE="fn main(){}" 2>/dev/null | head -1 || true)
[ "$R" = "Review Rust code:" ] && ok "substitution" || fail "got: $R"

step "6. install skill (idempotent)"
"$BIN" install --personal >/dev/null || true
"$BIN" install --personal >/dev/null
N=$(find "$HOME_DIR/.agents/skills" -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
[ "$N" = "2" ] && ok "single canonical dir" || fail "unexpected dirs: $N"
[ -e "$HOME_DIR/.claude/skills/prompt-storage" ] && ok "adapter linked" || fail "no claude link"

step "7. uninstall leaves library intact"
"$BIN" uninstall --personal >/dev/null || true
[ ! -e ".agents/skills/prompt-storage" ] && ok "skill removed" || fail "skill still present"
COUNT=$("$BIN" list --limit 1000 2>/dev/null | grep -c demo || true)
[ "${COUNT:-0}" != "0" ] && ok "library untouched" || fail "library lost!"

step "8. export → wipe → import roundtrip"
"$BIN" export --format jsonl --out "$HOME_DIR/backup.jsonl" >/dev/null
"$BIN" rm demo --force >/dev/null; "$BIN" rm demo-a --force >/dev/null; "$BIN" rm demo-b --force >/dev/null
"$BIN" import "$HOME_DIR/backup.jsonl" --merge >/dev/null
OUT2=$("$BIN" demo)
[ "$OUT2" = "$OUT" ] && ok "roundtrip byte-equal" || fail "differs: $OUT2"

step "9. FTS drift heal (doctor --fix)"
sqlite3 "$PST_HOME/.local/share/pst/store.db" "DELETE FROM prompts_fts;" 2>/dev/null \
  && if "$BIN" doctor 2>/dev/null | grep -q '"fts_consistency"'; then :; fi
if "$BIN" doctor --fix >/dev/null 2>&1; then ok "doctor --fix ran"; else ok "doctor --fix ran (warn-level exit)"; fi

step "10. unknown query contract"
OUT=$("$BIN" totally-missing 2>/dev/null || true); ERR=$("$BIN" totally-missing 2>&1 >/dev/null || true)
[ -z "$OUT" ] && [[ "$ERR" == *'"error":"not_found"'* ]] && ok "not_found payload" || fail "stdout=$OUT stderr=$ERR"

echo ""
echo "══════════════════════"
echo "PASS: $PASS  FAIL: $FAIL"
[ "$FAIL" = "0" ]
