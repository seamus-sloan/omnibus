#!/usr/bin/env bash
# Convert the coverage archive inside an .xcresult bundle to lcov on stdout,
# filtered to the app sources and emitted with repo-relative paths (what
# Codecov expects). Only line records (DA:) are produced — branch coverage
# isn't in xccov's line archive.
#
# Usage: xccov-to-lcov.sh <path.xcresult> [source-prefix]
#        source-prefix defaults to omnibus-ios/omnibus/ — test code and
#        anything outside it is dropped.
set -euo pipefail

xcresult="${1:?usage: xccov-to-lcov.sh <path.xcresult> [source-prefix]}"
prefix="${2:-omnibus-ios/omnibus/}"

xcrun xccov view --archive --file-list "$xcresult" | LC_ALL=C sort -u \
  | while IFS= read -r abs; do
    # The archive stores absolute build-machine paths; keep only files under
    # the source prefix and re-root them at the repo-relative prefix.
    rel="${abs#*/"$prefix"}"
    [ "$rel" = "$abs" ] && continue
    echo "SF:$prefix$rel"
    # Archive lines look like "  12: 3" (hit count) or "  7: *" (not
    # executable); subrange detail in brackets is ignored.
    xcrun xccov view --archive --file "$abs" "$xcresult" \
      | sed -n -E 's/^[[:space:]]*([0-9]+):[[:space:]]*([0-9]+).*/DA:\1,\2/p'
    echo "end_of_record"
  done
