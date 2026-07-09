#!/usr/bin/env bash
# Run a command inside a Nix dev shell WITHOUT paying the per-invocation
# `nix develop` cost (which re-copies the flake source into /nix/store and
# re-evaluates the flake every time).
#
# Usage: scripts/with-dev-env.sh <shell> <command> [args...]
#          e.g. scripts/with-dev-env.sh web scripts/dev-server-up.sh
#
# How: `nix print-dev-env .#<shell>` is captured once and cached, keyed on
# sha256(flake.nix + flake.lock + shell). The cache is sourced (which re-runs
# the shellHook, so per-workspace PORT / .env / sccache guards stay live) and
# the command exec'd. On a cache hit this skips Nix entirely — only a flake
# edit (or `nix store gc` collecting a referenced path) forces a rebuild.
#
# Escape hatches:
#   OMNIBUS_NO_ENV_CACHE=1   bypass the cache, run plain `nix develop`
#   just env-clear           delete the cache dir
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "usage: $0 <shell> <command> [args...]" >&2
  exit 1
fi
shell="$1"
shift

# Validate against the flake's devShells. `shell` is interpolated into the
# cache path and a `rm -f "$cache_dir/$shell-"*` glob, so an unconstrained
# value (`/`, `..`) could read/delete outside the cache dir.
case "$shell" in
  default | web | e2e | mobile | audit) ;;
  *)
    echo "with-dev-env: unknown shell '$shell' (expected: default|web|e2e|mobile|audit)" >&2
    exit 2
    ;;
esac

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# Bypass: straight to nix develop.
if [ -n "${OMNIBUS_NO_ENV_CACHE:-}" ]; then
  exec nix develop "$repo_root#$shell" --command "$@"
fi

sha256_hex() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1
  else shasum -a 256 | cut -d' ' -f1; fi
}

cache_dir="$repo_root/.claude/runtime/nix-env-cache"
key="$( (cat "$repo_root/flake.nix" "$repo_root/flake.lock"; echo "$shell") | sha256_hex | cut -c1-16 )"
cache_file="$cache_dir/$shell-$key.env.bash"

# Self-heal: a `nix store gc` can collect a store path the cached env
# references, leaving a poisoned cache. Spot-check the first /nix/store path.
cache_is_stale() {
  [ ! -f "$cache_file" ] && return 0
  local p
  p="$(grep -o '/nix/store/[^\"'\'' :]*' "$cache_file" | head -1 || true)"
  [ -n "$p" ] && [ ! -e "$p" ]
}

if cache_is_stale; then
  mkdir -p "$cache_dir"
  rm -f "$cache_dir/$shell-"*.env.bash   # drop superseded keys for this shell
  if ! nix print-dev-env "$repo_root#$shell" > "$cache_file.tmp" 2>"$cache_file.err"; then
    # Flake eval failed (dirty tree, network, etc.) — surface why, then fall
    # back to nix develop (which re-runs the same eval and would re-emit it).
    cat "$cache_file.err" >&2
    rm -f "$cache_file.tmp" "$cache_file.err"
    exec nix develop "$repo_root#$shell" --command "$@"
  fi
  rm -f "$cache_file.err"
  mv "$cache_file.tmp" "$cache_file"
fi

# Source the env (shellHook stdout → stderr so it can't corrupt a command's
# stdout contract, e.g. dev-server-up.sh's single OMNIBUS_DEV_READY line),
# then exec the command.
exec bash -c 'source "$1" >&2; shift; exec "$@"' _ "$cache_file" "$@"
