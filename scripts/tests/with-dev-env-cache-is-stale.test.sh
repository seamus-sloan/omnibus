#!/usr/bin/env bash
# Regression test for with-dev-env.sh's cache_is_stale() staleness check
# (issue #1421): a partial `nix store gc` can collect a later-referenced
# store path while an earlier one survives, and the check must still report
# the cache stale — not just spot-check the first path.
#
# No nix, no network: the function is extracted verbatim from the real
# script (so this tracks the shipped implementation, not a hand-copied
# duplicate) and exercised against synthetic cache files. Real /nix/store
# entries already present on this machine (from the Nix installer itself)
# stand in for "path exists"; fabricated paths stand in for "GC'd".
#
# Usage: scripts/tests/with-dev-env-cache-is-stale.test.sh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script="$repo_root/scripts/with-dev-env.sh"

func_src="$(sed -n '/^cache_is_stale() {/,/^}/p' "$script")"
if [ -z "$func_src" ]; then
  echo "FAIL: could not extract cache_is_stale() from $script" >&2
  exit 1
fi
eval "$func_src"

# Two store paths that genuinely exist on this box, standing in for
# survived-GC paths. A fabricated suffix stands in for a collected one.
existing_paths=(/nix/store/*/)
if [ "${#existing_paths[@]}" -lt 2 ]; then
  echo "SKIP: fewer than two /nix/store entries on this machine; cannot build fixtures" >&2
  exit 0
fi
exists_a="${existing_paths[0]%/}"
exists_b="${existing_paths[1]%/}"
missing="/nix/store/0000000000000000000000000000000-omnibus-1421-gc-test"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

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

verdict() {
  # cache_is_stale returns 0 (true) for "stale" and 1 (false) for "fresh",
  # matching its `if cache_is_stale; then rebuild; fi` call site.
  if cache_is_stale; then echo stale; else echo fresh; fi
}

# No cache file at all -> stale (nothing to spot-check).
cache_file="$tmp/missing.env.bash"
check "missing cache file is stale" stale "$(verdict)"

# Every referenced path survives -> fresh.
cache_file="$tmp/all-present.env.bash"
printf 'export PATH="%s:%s:$PATH"\n' "$exists_a" "$exists_b" >"$cache_file"
check "cache with only surviving paths is fresh" fresh "$(verdict)"

# First path survives, a LATER path was collected -> stale. This is the
# exact regression from #1421: the old check only looked at the first path
# (`head -1`) and would have reported "fresh" here.
cache_file="$tmp/later-missing.env.bash"
printf 'export PATH="%s:%s:$PATH"\n' "$exists_a" "$missing" >"$cache_file"
check "cache with a later collected path is stale" stale "$(verdict)"

# Only path was collected -> stale.
cache_file="$tmp/only-missing.env.bash"
printf 'export PATH="%s/bin:$PATH"\n' "$missing" >"$cache_file"
check "cache with its only path collected is stale" stale "$(verdict)"

echo "---"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
