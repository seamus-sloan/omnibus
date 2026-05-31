#!/usr/bin/env bash
# Restart THIS workspace's dev server cleanly. Useful after adding a sqlx
# migration (the running server boots its migrations at startup), or when
# `dx serve` has wedged on a compile error and `dev-up` exits 2 telling
# you to bounce.
#
# Always identity-checked via dev-server-down.sh — never touches a
# sibling workspace's server.
#
# Exits:
#   0 — server stopped + restarted; OMNIBUS_DEV_READY action=restarted on stdout.
#   non-zero — propagates from dev-server-up.sh on failure to come back up.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"$SCRIPT_DIR/dev-server-down.sh" >/dev/null || true

# dev-server-up.sh emits its own OMNIBUS_DEV_READY line on stdout with
# action=started; rewrite that to action=restarted so callers see the
# bounce semantic. Everything else (stderr, exit code) passes through.
"$SCRIPT_DIR/dev-server-up.sh" | sed 's/action=started/action=restarted/'
