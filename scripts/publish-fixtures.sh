#!/usr/bin/env bash
# Publish a new public-domain fixtures release. Counterpart to
# scripts/fetch-fixtures.sh — run this after adding/updating files under
# test_data/{epubs,audiobooks}/public_domain/ (they're gitignored, so the
# bytes never land in git; the release asset is the source of truth).
#
# Usage: scripts/publish-fixtures.sh v2
#
# What it does:
#   1. Refuses to reuse an existing fixtures-<vN> tag (releases are immutable;
#      old tags stay so old commits' pinned URLs keep resolving).
#   2. Tars the on-disk public_domain dirs (null-safe: paths contain spaces),
#      round-trips the tarball against the source files, and uploads it via
#      `gh release create`.
#   3. Re-downloads the published asset, verifies the hash survived upload,
#      and prints the exact pin lines to paste into scripts/fetch-fixtures.sh
#      plus a reminder to bump the actions/cache key in CI.
set -euo pipefail

if [ $# -ne 1 ] || ! [[ "$1" =~ ^v[0-9]+$ ]]; then
  echo "usage: $0 v<N>   (e.g. $0 v2)" >&2
  exit 1
fi
version="fixtures-$1"
asset="omnibus-fixtures-$1.tar.gz"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if gh release view "$version" >/dev/null 2>&1; then
  echo "publish-fixtures: release $version already exists — releases are immutable, bump N." >&2
  exit 1
fi

# Warn when the local set isn't the currently-pinned version: you may be
# about to tar a stale or partial checkout of the fixtures.
pinned="$(sed -n 's/^FIXTURES_VERSION="\(.*\)"/\1/p' scripts/fetch-fixtures.sh)"
marker="test_data/.fixtures-version"
if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$pinned" ]; then
  echo "WARNING: $marker doesn't match the pinned $pinned — local fixtures may be stale." >&2
  echo "         (Continue only if you intentionally rebuilt the set.)" >&2
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Building $asset from on-disk public_domain dirs..." >&2
# -print0 | tar --null: audiobook paths contain spaces.
find test_data/epubs/public_domain test_data/audiobooks/public_domain \
    -type f ! -name '.DS_Store' -print0 | sort -z \
  | tar -czf "$tmp/$asset" --null -T - --no-recursion

echo "Round-trip check..." >&2
mkdir "$tmp/rt"
tar -xzf "$tmp/$asset" -C "$tmp/rt"
while IFS= read -r -d '' f; do
  cmp -s "$f" "$tmp/rt/$f" || { echo "publish-fixtures: round-trip mismatch: $f" >&2; exit 1; }
done < <(find test_data/epubs/public_domain test_data/audiobooks/public_domain \
             -type f ! -name '.DS_Store' -print0)

local_sha="$(sha256_of "$tmp/$asset")"
echo "Uploading $version ($(du -h "$tmp/$asset" | cut -f1 | tr -d ' '))..." >&2
gh release create "$version" "$tmp/$asset" \
  --title "Test fixtures $1" \
  --notes "Public-domain test fixtures. Fetched by scripts/fetch-fixtures.sh (\`just fixtures\`)." \
  --latest=false

echo "Verifying published asset hash..." >&2
url="https://github.com/seamus-sloan/omnibus/releases/download/$version/$asset"
curl -fsSL --retry 3 -o "$tmp/published.tar.gz" "$url"
published_sha="$(sha256_of "$tmp/published.tar.gz")"
if [ "$published_sha" != "$local_sha" ]; then
  echo "publish-fixtures: published hash differs from local — investigate before pinning." >&2
  exit 1
fi

cat >&2 <<EOF

Published $version. Now update the pins:

  scripts/fetch-fixtures.sh:
    FIXTURES_VERSION="$version"
    FIXTURES_URL="$url"
    FIXTURES_SHA256="$published_sha"

  CI (both .github/workflows/rust.yml and e2e.yml):
    actions/cache key: $version

  Then: refresh test_data/*/README.md fixture tables and, if EPUB metadata
  changed, ui_tests/playwright/tests/fixtures/*.ts expectations.
EOF
