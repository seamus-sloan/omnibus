#!/usr/bin/env bash
# Report and persist the OMNIBUS_EXPLORE_* settings the swarm cannot run
# without.
#
#   env.sh check            # names what is missing, one key per line
#   env.sh set KEY VALUE    # writes one answer into the repo `.env`
#
# The runner asks the user for a missing value and writes it back here rather
# than exporting it for one shell: the next run is a different session, and a
# value that only lived in this one gets asked for again.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
ENV_FILE="$ROOT/.env"

# Required, not merely documented. OMNIBUS_EXPLORE_JOURNAL_DIR has a fallback
# in owned.sh, but it falls back to `.claude/runtime/` — gitignored and
# per-worktree — so running without it means a `wt switch` orphans every book
# earlier runs uploaded. That is a silent data-ownership loss, so it is asked
# for like the other two.
REQUIRED=(OMNIBUS_EXPLORE_URL OMNIBUS_EXPLORE_ADMIN OMNIBUS_EXPLORE_JOURNAL_DIR)

# Snapshotting and the server-log half of the report need these; a run without
# them still works and says what it could not read. Settable, never prompted.
OPTIONAL=(
  OMNIBUS_EXPLORE_SSH_HOST OMNIBUS_EXPLORE_REMOTE_DIR OMNIBUS_EXPLORE_REMOTE_LOG_DIR
  OMNIBUS_EXPLORE_SNAPSHOT_DIR OMNIBUS_EXPLORE_SNAPSHOT_KEEP
)

# An exported value wins over `.env`, matching explore::load_env and
# audit_lib.env — so a caller pointing at another instance is not told to
# write that instance into the file.
#
# `head -1` is not a coin toss: explore::load_env exports the *first*
# occurrence, and it is what gates the run at preflight. Reading the last one
# here would let `check` pass on a value the run never uses.
explore_env::value() {
  local key="$1"
  if [ -n "${!key-}" ]; then printf '%s' "${!key}"; return; fi
  [ -f "$ENV_FILE" ] || return 0
  grep -E "^${key}=" "$ENV_FILE" | head -1 | cut -d= -f2- || true
}

explore_env::count() {
  [ -f "$ENV_FILE" ] || { echo 0; return; }
  grep -cE "^${1}=" "$ENV_FILE" || true
}

# Named when missing, and also when duplicated. A duplicate is not cosmetic:
# lib.sh takes the first occurrence and audit_lib.env the last, so the shell
# and Python halves of one run would target different instances — and the
# shell half is the one that deletes books. Naming it sends the key back
# through `set`, which collapses the duplicate.
explore_env::check() {
  local key
  for key in "${REQUIRED[@]}"; do
    if [ -z "$(explore_env::value "$key")" ]; then
      echo "$key"
    elif [ "$(explore_env::count "$key")" -gt 1 ]; then
      echo "$key"
    fi
  done
}

explore_env::known() {
  local key
  for key in "${REQUIRED[@]}" "${OPTIONAL[@]}"; do
    [ "$key" = "$1" ] && return 0
  done
  return 1
}

explore_env::set() {
  local key="${1-}" value="${2-}"
  explore_env::known "$key" \
    || { echo "unknown key: $key — nothing reads it; see .env.example" >&2; return 2; }
  [ -n "$value" ] || { echo "$key needs a value" >&2; return 2; }
  # `.env` is parsed line-wise by four different readers, none of which handle
  # a quoted or wrapped value. Refuse rather than write something that loads
  # back as a different string.
  case "$value" in
    *[[:space:]]*) echo "$key value must not contain whitespace" >&2; return 2 ;;
    \#*)           echo "$key value must not start with #" >&2; return 2 ;;
  esac

  if [ ! -f "$ENV_FILE" ]; then
    # OMNIBUS_EXPLORE_ADMIN is a live credential; never create the file
    # world-readable and then narrow it.
    (umask 077; : > "$ENV_FILE")
  fi

  python3 - "$ENV_FILE" "$key" "$value" <<'PY'
import pathlib
import sys

path, key, value = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
lines = path.read_text(encoding="utf-8", errors="replace").splitlines()

# Rewrite the first assignment in place — commented or live — so the annotated
# block .env.example ships keeps its comments instead of growing a second copy
# of the key at the bottom. Later duplicates are dropped: lib.sh takes the
# first occurrence and audit_lib.env takes the last, so a file carrying two
# would mean the shell and Python halves of one run disagree about the target.
prefixes = (f"{key}=", f"#{key}=", f"# {key}=")
kept, done = [], False
for line in lines:
    if line.lstrip().startswith(prefixes):
        if done:
            continue
        kept.append(f"{key}={value}")
        done = True
    else:
        kept.append(line)
if not done:
    kept.append(f"{key}={value}")

path.write_text("\n".join(kept) + "\n", encoding="utf-8")
PY
  # The value is a credential in at least one case — report the key alone.
  echo "wrote $key to $ENV_FILE"
}

case "${1-}" in
  check) explore_env::check ;;
  set)   shift; explore_env::set "${1-}" "${2-}" ;;
  *)     echo "usage: env.sh {check|set KEY VALUE}" >&2; exit 2 ;;
esac
