#!/usr/bin/env bash
# Tests for rewrite_origin_port() in scripts/dev-serve-fg.sh.
#
# The serve pane inherits OMNIBUS_PUBLIC_ORIGIN from the shellHook, which
# names the worktree's *base* port. Once the walk lands somewhere else, an
# inherited entry that kept the old port stays allowlisted — and because
# browser cookies are not port-scoped, a sibling worktree's page on that
# origin could POST into this server past origin_check. Every inherited
# entry must therefore come out carrying the port we actually bound.
#
# The function is extracted verbatim from the real script so this tracks
# the shipped implementation rather than a hand-copied duplicate.
#
# Usage: scripts/tests/dev-serve-origin-rewrite.test.sh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script="$repo_root/scripts/dev-serve-fg.sh"

func_src="$(sed -n '/^rewrite_origin_port() {/,/^}/p' "$script")"
if [ -z "$func_src" ]; then
    echo "FAIL: could not extract rewrite_origin_port() from $script" >&2
    exit 1
fi
eval "$func_src"

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

# The case this exists for: the shellHook's base-port origin must not stay
# allowlisted once the walk moves us off the base.
check "base-port origin is re-pointed at the bound port" \
    "http://localhost:3901" "$(rewrite_origin_port http://localhost:3900 3901)"

# An operator's LAN origin from .env keeps its host — that's why the value
# is inherited at all rather than discarded.
check "LAN host survives, its port does not" \
    "http://192.168.1.5:3901" "$(rewrite_origin_port http://192.168.1.5:3000 3901)"

check "https scheme is preserved" \
    "https://dev.example.com:3901" "$(rewrite_origin_port https://dev.example.com 3901)"

check "IPv6 literal keeps its brackets" \
    "http://[::1]:3901" "$(rewrite_origin_port 'http://[::1]:3000' 3901)"

# An Origin header is scheme+host+port; a path would never match one.
check "path component is dropped" \
    "http://host:3901" "$(rewrite_origin_port http://host:3000/path 3901)"

check "trailing slash is dropped" \
    "http://host:3901" "$(rewrite_origin_port http://host:3000/ 3901)"

# Better to pass an unparseable entry through untouched than to mangle it
# into something that silently matches nothing.
check "scheme-less garbage passes through unchanged" \
    "garbage" "$(rewrite_origin_port garbage 3901)"

echo "---"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
