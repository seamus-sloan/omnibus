#!/usr/bin/env bash
# Fetch the public-domain test fixtures (EPUBs + audiobooks) that are NOT
# tracked in git. They live as a GitHub release asset so the ~156 MB of
# binaries stays out of the git-tracked tree — which `nix develop` copies
# into /nix/store on every working-copy change.
#
# What it does:
#   1. No-ops instantly when test_data/.fixtures-version matches the pinned
#      version and a sentinel file exists.
#   2. Otherwise downloads the pinned release tarball, verifies its sha256,
#      and extracts into test_data/{epubs,audiobooks}/public_domain/.
#
# Runs OUTSIDE any nix shell on purpose (curl + tar only). Invoked by
# `just fixtures` (and transitively `just test`); CI calls it directly.
# To publish a new fixtures release, see scripts/publish-fixtures.sh.
set -euo pipefail

FIXTURES_VERSION="fixtures-v1"
FIXTURES_URL="https://github.com/seamus-sloan/omnibus/releases/download/${FIXTURES_VERSION}/omnibus-fixtures-v1.tar.gz"
FIXTURES_SHA256="8e58b6c1c363e248115338d356841bf90536574c69474a2a8f7b538f107d7bbf"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
marker="$repo_root/test_data/.fixtures-version"
# One committed-era file per media type — catches a wiped dir behind a stale marker.
sentinel_epub="$repo_root/test_data/epubs/public_domain/count_of_monte_cristo.epub"
sentinel_audio="$repo_root/test_data/audiobooks/public_domain/Arthur Conan Doyle/A Womans Love.m4b"

if [ -f "$marker" ] && [ "$(cat "$marker")" = "$FIXTURES_VERSION" ] \
   && [ -f "$sentinel_epub" ] && [ -f "$sentinel_audio" ]; then
  exit 0
fi

# macOS ships shasum, Linux CI ships sha256sum; nix shells vary.
sha256_check() {
  if command -v sha256sum >/dev/null 2>&1; then
    echo "$1  $2" | sha256sum -c - >/dev/null
  else
    echo "$1  $2" | shasum -a 256 -c - >/dev/null
  fi
}

echo "Fetching test fixtures ($FIXTURES_VERSION, ~156 MB)..." >&2
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/fixtures.tar.gz" "$FIXTURES_URL"
sha256_check "$FIXTURES_SHA256" "$tmp/fixtures.tar.gz" || {
  echo "fetch-fixtures: sha256 mismatch for $FIXTURES_URL" >&2
  exit 1
}
# Paths inside the tarball are repo-relative (test_data/...). Exclude macOS
# AppleDouble sidecars / .DS_Store defensively: a tarball built on macOS
# without COPYFILE_DISABLE=1 carries `._foo.epub` files that Linux tar
# extracts literally, and the ebook scanner would count them as fixtures.
tar -xzf "$tmp/fixtures.tar.gz" -C "$repo_root" --exclude='._*' --exclude='.DS_Store'
echo "$FIXTURES_VERSION" > "$marker"
echo "Fixtures ready ($FIXTURES_VERSION)." >&2
