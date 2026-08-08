#!/usr/bin/env bash
# Foreground dev-server launcher for the `just serve` / `just serve-pc`
# multiplexer panes.
#
# The panes used to hardcode `--port 3000`, so every worktree's server
# raced for one port. This resolves a free port inside THIS worktree's
# window first (same walk `just dev-up` uses), publishes it to
# .claude/runtime/, then execs `dx serve` so the pane still owns the
# process and Ctrl-C still stops it.
#
# Override the window's base with PORT; override the bind address with
# OMNIBUS_DEV_ADDR.

set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
# shellcheck source=scripts/lib/dev-port.sh
source scripts/lib/dev-port.sh

# --free-only: a pane is being asked to *run* a server, so adopting one
# that's already up (from `just dev-up`, say) isn't an answer.
if ! choice="$(choose_port --free-only)"; then
    echo "serve: ports $START_PORT-$END_PORT are all in use." >&2
    echo "       Free one, or set PORT to a different window base." >&2
    exit 1
fi
port="${choice%%:*}"

# --addr 0.0.0.0 binds all interfaces so other devices on the LAN can reach
# the dev server. Cookie-authed POSTs from a LAN origin still need that
# origin in OMNIBUS_PUBLIC_ORIGIN plus OMNIBUS_SECURE_COOKIES=0 — see
# .env.example. Trusted networks only: this exposes the dev server (auth
# endpoints included) to the whole LAN, so don't run it on public Wi-Fi.
addr="${OMNIBUS_DEV_ADDR:-0.0.0.0}"

# Re-point an inherited origin at the port we actually landed on, keeping
# its scheme and host. Anything without a scheme is passed through rather
# than mangled.
rewrite_origin_port() {
    local origin="${1%/}" to="$2" scheme rest host
    case "$origin" in
        *://*) ;;
        *) printf '%s' "$origin"; return ;;
    esac
    scheme="${origin%%://*}"
    rest="${origin#*://}"
    rest="${rest%%/*}"
    case "$rest" in
        \[*\]*) host="${rest%%\]*}]" ;;  # IPv6 literal — keep the brackets
        *) host="${rest%%:*}" ;;
    esac
    printf '%s://%s:%s' "$scheme" "$host" "$to"
}

# origin_check compares the browser's Origin against this allowlist, so every
# entry has to name the port we landed on. Inheriting the value verbatim
# would leave the shellHook's *base*-port origin allowlisted — and browser
# cookies aren't port-scoped, so a sibling worktree's page on that origin
# could then POST into this server. Rewriting each inherited entry onto our
# port keeps an operator's LAN host from .env while dropping its stale port.
origins="http://localhost:$port,http://127.0.0.1:$port"
if [ -n "${OMNIBUS_PUBLIC_ORIGIN:-}" ]; then
    IFS=',' read -ra inherited <<<"$OMNIBUS_PUBLIC_ORIGIN"
    for entry in "${inherited[@]}"; do
        entry="${entry//[[:space:]]/}"
        [ -n "$entry" ] || continue
        rewritten="$(rewrite_origin_port "$entry" "$port")"
        case ",$origins," in
            *",$rewritten,"*) continue ;;  # already covered
        esac
        origins="$origins,$rewritten"
    done
fi

export PORT="$port"
export OMNIBUS_PUBLIC_ORIGIN="$origins"

write_runtime_files "$port"

echo "serve: starting dx serve on port $port (http://localhost:$port)" >&2
exec dx serve --platform web --fullstack \
    --port "$port" --addr "$addr" --package omnibus
