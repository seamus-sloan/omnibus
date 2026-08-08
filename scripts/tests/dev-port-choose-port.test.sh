#!/usr/bin/env bash
# Tests for choose_port() in scripts/lib/dev-port.sh — the walk that keeps
# two worktrees' dev servers off the same port.
#
# No network, no dx serve: the real library is sourced (so this tracks the
# shipped implementation rather than a hand-copied duplicate) and only its
# two I/O primitives — is_port_free and probe_health — are stubbed. Each
# scenario declares which ports are held and what /api/_health says on
# each, then asserts the port the walk settles on.
#
# Usage: scripts/tests/dev-port-choose-port.test.sh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

PORT=3000 source scripts/lib/dev-port.sh

# The walk compares a server's reported repo_root against MY_REPO_ROOT to
# tell "my server" from "a sibling's". Pin both so the assertions don't
# depend on where the suite is checked out.
MY_REPO_ROOT="/wt/mine"
SIBLING_ROOT="/wt/sibling"

declare -A HELD    # port -> health body ("" = held by a non-omnibus process)

# --- stubs for the two primitives that touch the outside world ----------
is_port_free() { [ -z "${HELD[$1]+set}" ]; }
probe_health() { echo "${HELD[$1]:-}"; }

omnibus_body() { echo "{\"app\":\"omnibus\",\"repo_root\":\"$1\"}"; }

pass=0
fail=0

check() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" = "$expected" ]; then
        echo "PASS: $desc"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc (expected $expected, got $actual)" >&2
        fail=$((fail + 1))
    fi
}

# choose_port writes skip notices to stderr; keep them out of the verdict.
verdict() { choose_port "$@" 2>/dev/null || echo "exhausted"; }

# Nothing listening -> take the window base.
HELD=()
check "empty window takes the base port" "3000:free" "$(verdict)"

# Our own healthy server on the base port -> reuse it (dev-up's contract).
HELD=([3000]="$(omnibus_body /wt/mine)")
check "own server is reused by default" "3000:reuse" "$(verdict)"

# Same state, but a foreground pane is being asked to *run* a server, so
# adopting the one already up is not an answer — walk past it.
check "own server is walked past under --free-only" "3001:free" "$(verdict --free-only)"

# A sibling worktree's omnibus on our base port must never be adopted: it's
# different code against a different DB.
HELD=([3000]="$(omnibus_body /wt/sibling)")
check "sibling's server is skipped, not reused" "3001:free" "$(verdict)"
check "sibling's server is skipped under --free-only" "3001:free" "$(verdict --free-only)"

# Something that isn't omnibus at all (a stray dev server, a proxy).
HELD=([3000]="" [3001]="")
check "foreign processes are walked past" "3002:free" "$(verdict)"

# Our own server sitting above a sibling's — the walk must pass the sibling
# and still recognise ours rather than stopping at the first omnibus it sees.
HELD=([3000]="$(omnibus_body /wt/sibling)" [3001]="$(omnibus_body /wt/mine)")
check "own server found past a sibling's" "3001:reuse" "$(verdict)"

# Every port in the window taken by processes we must not disturb.
HELD=()
for p in $(seq 3000 3009); do HELD[$p]=""; done
check "exhausted window fails rather than picking a held port" "exhausted" "$(verdict)"

# A full window of siblings is equally exhausted — no silent adoption.
HELD=()
for p in $(seq 3000 3009); do HELD[$p]="$(omnibus_body /wt/sibling)"; done
check "window full of siblings is exhausted" "exhausted" "$(verdict)"

echo "---"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
