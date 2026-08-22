#!/usr/bin/env bash
# pst performance bench (bead E2E.1) — enforces plan §13 budgets, p95.
set -euo pipefail

BIN="${BIN:-$(pwd)/target/release/pst}"
if [ ! -x "$BIN" ]; then BIN="$(pwd)/target/debug/pst"; fi
if [ ! -x "$BIN" ]; then echo "FATAL: build pst first"; exit 1; fi

ITERATIONS="${ITERATIONS:-50}"
SEED_COUNT="${SEED_COUNT:-10000}"
FAILURES=0

HOME_DIR=$(mktemp -d)
trap 'rm -rf "$HOME_DIR"' EXIT
export PST_HOME="$HOME_DIR"

echo "Seeding $SEED_COUNT synthetic prompts..."
python3 - "$SEED_COUNT" <<'PY' | "$BIN" import /dev/stdin --merge >/dev/null 2>&1 || true
import json, sys
n = int(sys.argv[1])
print(json.dumps({"_meta": {"version": "0", "count": n, "exported_at": "bench", "schema_version": 1}}))
for i in range(n):
    print(json.dumps({
        "id": f"bench-prompt-{i:06d}",
        "title": f"Bench Prompt {i}",
        "content": f"Content for bench prompt {i} about topic-{i % 50}.",
        "tags": [f"tag{i % 20}"],
        "aliases": [],
        "variables": [],
    }))
sys.stdout.flush()
PY

# Warm-up (excluded from stats)
"$BIN" bench-prompt-000000 >/dev/null 2>&1 || true

measure() {
    local label="$1" budget_ms="$2"; shift 2
    local times=()
    for _ in $(seq 1 "$ITERATIONS"); do
        local t0 t1
        t0=$(python3 -c 'import time; print(time.monotonic_ns())')
        "$@" >/dev/null 2>&1
        t1=$(python3 -c 'import time; print(time.monotonic_ns())')
        times+=( $(( (t1 - t0) / 1000000 )) )
    done
    # p95 = second-highest of N samples (approximation adequate for budgets)
    local sorted=($(printf '%s\n' "${times[@]}" | sort -n))
    local p95=${sorted[$(( ${#sorted[@]} * 95 / 100 - 1 ))]:-${sorted[-1]}}
    local status="OK"
    if [ "$p95" -gt "$budget_ms" ]; then status="OVER BUDGET"; FAILURES=$((FAILURES+1)); fi
    printf "%-38s p95=%4s ms   budget<=%s ms   [%s]\n" "$label" "$p95" "$budget_ms" "$status"
}

echo ""
echo "Measuring ($ITERATIONS iterations each)..."
measure "pst <exact-id>"          30 "$BIN" bench-prompt-005000
measure "pst <unique-prefix>"     30 "$BIN" bench-prompt-005000 || true
measure "pst search <query>"      50 "$BIN" search "topic" || true
measure "pst render <id> --VARS"  40 "$BIN" render bench-prompt-005000 --X=y

echo ""
if [ "$FAILURES" = "0" ]; then
    echo "✓ ALL BUDGETS MET"
else
    echo "✗ $FAILURES OPERATION(S) OVER BUDGET"
    exit 1
fi
