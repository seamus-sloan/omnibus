#!/usr/bin/env bash
# Shared helpers for the agentic-exploration scripts. Source, don't execute.
#
# Resolves the target instance and the one credential that must persist across
# sessions (OMNIBUS_EXPLORE_ADMIN). Everything else — per-agent passwords, run
# ids, journals — is derived per run and never stored.

set -euo pipefail

# Load .env from the repo root without letting it clobber anything already
# exported (an explicit env var wins, matching the shellHook's precedence).
explore::load_env() {
  local root
  root="$(git rev-parse --show-toplevel)"
  if [ -f "$root/.env" ]; then
    while IFS= read -r line; do
      case "$line" in ''|\#*) continue ;; esac
      local key="${line%%=*}"
      [ -n "${!key-}" ] && continue
      export "${line?}"
    done < <(grep -E '^OMNIBUS_EXPLORE_[A-Z_]+=' "$root/.env" || true)
  fi

  : "${OMNIBUS_EXPLORE_URL:?set OMNIBUS_EXPLORE_URL in .env — see .env.example}"
  : "${OMNIBUS_EXPLORE_ADMIN:?set OMNIBUS_EXPLORE_ADMIN=user:password in .env — see .env.example}"

  EXPLORE_URL="${OMNIBUS_EXPLORE_URL%/}"
  EXPLORE_ADMIN_USER="${OMNIBUS_EXPLORE_ADMIN%%:*}"
  EXPLORE_ADMIN_PASS="${OMNIBUS_EXPLORE_ADMIN#*:}"
  export EXPLORE_URL EXPLORE_ADMIN_USER EXPLORE_ADMIN_PASS
}

# Every mutating request needs an Origin the server's origin_check allows, or
# it 403s regardless of session — see auth::origin_check.
explore::curl() {
  curl -sS --max-time 30 -H "Origin: $EXPLORE_URL" "$@"
}

# Log in as admin, leaving a cookie jar at $EXPLORE_JAR. Fails loudly: a bad
# admin credential must not be mistaken for "the instance is down".
explore::login_admin() {
  EXPLORE_JAR="$(mktemp -t omni-explore-jar)"
  export EXPLORE_JAR
  # The jar holds a live admin session token. Remove it on any exit — a token
  # left in /tmp outlives the run and is a credential, not a temp file.
  trap 'rm -f "${EXPLORE_JAR-}"' EXIT INT TERM
  local code
  code=$(explore::curl -c "$EXPLORE_JAR" -o /dev/null -w '%{http_code}' \
    -X POST "$EXPLORE_URL/api/auth/login" \
    -H 'Content-Type: application/json' \
    -d "$(printf '{"username":%s,"password":%s}' \
          "$(explore::json_str "$EXPLORE_ADMIN_USER")" \
          "$(explore::json_str "$EXPLORE_ADMIN_PASS")")")
  if [ "$code" != "200" ]; then
    echo "admin login failed (HTTP $code) at $EXPLORE_URL — check OMNIBUS_EXPLORE_ADMIN" >&2
    return 1
  fi
}

# JSON-encode a string so passwords containing quotes or backslashes survive.
explore::json_str() { python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }

# A readable, strong password. Never persisted — provisioning rotates it.
explore::gen_password() {
  python3 - <<'PY'
import secrets
w = ["harbor","thistle","lantern","quarry","meadow","cinder","falcon","bramble",
     "vellum","orchard","tundra","marlowe","kestrel","juniper"]
print("-".join(secrets.choice(w) for _ in range(3)) + "-" + str(secrets.randbelow(9000) + 1000))
PY
}

explore::health() {
  explore::curl -o /dev/null -w '%{http_code}' "$EXPLORE_URL/api/_health"
}
