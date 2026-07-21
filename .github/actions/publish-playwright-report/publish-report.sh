#!/usr/bin/env bash
# Publish a merged HTML report into the gh-pages branch at
# <branch>/<sha>/<subdir>/, preserving every other report already there, and
# point the site root at the latest report on the trunk branch.
#
# Invoked by action.yml's "Publish report to gh-pages" step, which supplies:
#   GH_TOKEN PAGES_BASE_URL GH_PAGES_BRANCH MAIN_BRANCH REPORT_SUBDIR
#   REPORT_TITLE REPORT_DIR REPO BRANCH_NAME COMMIT_SHA
#   GITHUB_WORKSPACE GITHUB_STEP_SUMMARY — set by the runner
set -euo pipefail

# Attribute the auto-generated report commits to the GitHub Actions bot — the
# canonical actor name + noreply address GitHub uses for it — so they show the
# bot identity/avatar rather than a real person.
COMMIT_USER_NAME="github-actions[bot]"
COMMIT_USER_EMAIL="41898282+github-actions[bot]@users.noreply.github.com"

cd "$GITHUB_WORKSPACE"

# Resolve the report source to an absolute path (report-dir may be
# workspace-relative) before we cd into the clone below.
REPORT_SRC="$(cd "$REPORT_DIR" && pwd)"

DEST="${BRANCH_NAME}/${COMMIT_SHA}/${REPORT_SUBDIR}"
AUTH_URL="https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"

# Clone the existing gh-pages tree; create an orphan branch if it's the first run
# and the branch doesn't exist yet.
if git clone --depth 1 --branch "$GH_PAGES_BRANCH" "$AUTH_URL" gh-pages 2>/dev/null; then
  cd gh-pages
else
  git clone --depth 1 "$AUTH_URL" gh-pages
  cd gh-pages
  git checkout --orphan "$GH_PAGES_BRANCH"
  git rm -rfq . || true
fi
git config user.name "$COMMIT_USER_NAME"
git config user.email "$COMMIT_USER_EMAIL"

# Replace only this run's folder; leave all other reports intact.
rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$REPORT_SRC/." "$DEST/"

# Write a meta-refresh redirect page. $1 = relative target URL, $2 = output file.
# Relative URLs so they work under a project-pages base path.
write_redirect() {
  printf '<!doctype html>\n<meta charset="utf-8">\n<meta http-equiv="refresh" content="0; url=%s">\n<title>%s</title>\n<a href="%s">Latest %s →</a>\n' "$1" "$REPORT_TITLE" "$1" "$REPORT_TITLE" >"$2"
}

# Per-branch pointer: <base>/<branch>/ → this branch's newest report. Updated
# every run, so a branch's latest report is reachable without knowing the sha.
write_redirect "./${COMMIT_SHA}/${REPORT_SUBDIR}/" "${BRANCH_NAME}/index.html"

# Track the latest trunk report in a sentinel file so the root can always point
# at it — even on a PR run, which must not move the root off trunk.
if [ "$BRANCH_NAME" = "$MAIN_BRANCH" ]; then
  printf '%s\n' "$DEST" >.main-latest
fi
# Keep a root index every run so the site root never 404s: redirect to the latest
# trunk report if one exists, else a small landing page until the first trunk run
# publishes.
if [ -f .main-latest ]; then
  write_redirect "./$(cat .main-latest)/" "index.html"
else
  printf '<!doctype html>\n<meta charset="utf-8">\n<title>%s</title>\n<h1>%s</h1>\n<p>No <code>%s</code> report has been published yet. Reports are published per branch; this page will redirect to the latest <code>%s</code> report once one exists.</p>\n' "$REPORT_TITLE" "$REPORT_TITLE" "$MAIN_BRANCH" "$MAIN_BRANCH" >"index.html"
fi

# Serve the report assets verbatim (no Jekyll processing of _ paths).
touch .nojekyll

# Link both URLs from the run's summary so the report is one click from the
# Actions page — no sha-hunting.
{
  echo "### 📊 ${REPORT_TITLE}"
  echo ""
  echo "- **This run:** ${PAGES_BASE_URL}/${DEST}/"
  echo "- **Latest for \`${BRANCH_NAME}\`:** ${PAGES_BASE_URL}/${BRANCH_NAME}/"
} >>"$GITHUB_STEP_SUMMARY"

git add -A
git commit -q -m "test report: ${BRANCH_NAME}@${COMMIT_SHA:0:9}" || {
  echo "no changes to publish"
  exit 0
}
git push "$AUTH_URL" HEAD:"$GH_PAGES_BRANCH"
