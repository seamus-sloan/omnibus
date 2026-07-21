#!/usr/bin/env bash
# Merge the per-shard Playwright blob reports into a single HTML report.
#
# Invoked by action.yml's "Merge blob reports" step, which supplies:
#   PLAYWRIGHT_DIR  — repo-relative path to the Playwright project
#   GITHUB_WORKSPACE — set by the runner
# The blob artifacts were downloaded to $GITHUB_WORKSPACE/all-blob-reports by the
# preceding download-artifact step.
set -euo pipefail

# merge-reports needs @playwright/test but no browser binaries; skip the ~150 MB
# Chromium download the postinstall would otherwise run.
export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1

cd "${GITHUB_WORKSPACE}/${PLAYWRIGHT_DIR}"
npm ci
npx playwright merge-reports --reporter html "${GITHUB_WORKSPACE}/all-blob-reports"
