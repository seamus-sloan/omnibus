#!/usr/bin/env bash
# Print the uuids an actor owns, comma-separated, for `driver.sh guard`.
#
# Ownership is provenance: you own a book if you added it, in *any* run. So
# this reads every journal, not just the current one — which is also why the
# exploration accounts keep stable usernames (see provision.sh).
#
# Usage: owned.sh <actor>            e.g. owned.sh agent-1

set -euo pipefail
actor="${1:?usage: owned.sh <actor>}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"

# The journal is the ownership ledger, so it must outlive any one checkout.
# `.claude/runtime/` is gitignored and therefore *per-worktree*: leaving the
# journals there means a `wt switch` silently orphans every book previously
# uploaded, because no `book.add` entry can be found for them any more. Pin
# OMNIBUS_EXPLORE_JOURNAL_DIR (see .env.example) to somewhere outside the
# worktrees; the in-repo path remains the fallback for a single checkout.
if [ -z "${OMNIBUS_EXPLORE_JOURNAL_DIR-}" ] && [ -f "$ROOT/.env" ]; then
  raw="$(grep -E '^OMNIBUS_EXPLORE_JOURNAL_DIR=' "$ROOT/.env" | tail -1 | cut -d= -f2- || true)"
  # .env values are literal text: strip surrounding quotes and expand $VARS and
  # a leading ~ ourselves. Without this, the `$HOME/...` form .env.example
  # recommends resolves to a directory that does not exist, owned.sh returns an
  # empty list, and the guard then refuses an agent's own books.
  raw="${raw%\"}"; raw="${raw#\"}"; raw="${raw%\'}"; raw="${raw#\'}"
  raw="${raw/#\~/$HOME}"
  OMNIBUS_EXPLORE_JOURNAL_DIR="$(eval printf '%s' "\"$raw\"")"
fi
JOURNALS="${OMNIBUS_EXPLORE_JOURNAL_DIR:-$ROOT/.claude/runtime/explore}"

python3 - "$actor" "$JOURNALS" <<'PY'
import json, pathlib, sys

actor, root = sys.argv[1], pathlib.Path(sys.argv[2])
owned = []
for journal in sorted(root.glob("*/journal.jsonl")):
    with journal.open() as fh:
      for line in fh:                      # stream: a long run's journal is large
        line = line.strip()
        if not line:
            continue
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            # A torn line is not a reason to hand back a shorter ownership
            # list — that would silently un-own a book. Fail loudly instead.
            sys.exit(f"unparseable journal line in {journal}: {line[:80]}")
        if e.get("actor") == actor and e.get("action") == "book.add" \
                and e.get("outcome") == "ok" and e.get("target"):
            owned.append(e["target"])
print(",".join(dict.fromkeys(owned)))
PY
