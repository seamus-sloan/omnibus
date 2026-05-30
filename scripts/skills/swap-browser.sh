#!/usr/bin/env bash
# Swap the active browser MCP referenced in .claude/skills/ui-validate/SKILL.md
# between Chrome DevTools MCP (the documented default) and Claude Preview MCP.
#
# Idempotent: running with the same argument twice is a no-op.
# Round-trip safe: the substitution table is 1:1 between the two MCPs for
# every tool ui-validate actually uses, so `chrome-devtools → preview →
# chrome-devtools` is byte-identical. (We avoid `navigate_page` in the
# skill body and use `evaluate_script` with `location.href=`/`reload()`
# instead, so the mapping stays unambiguous.)
#
# Usage: scripts/skills/swap-browser.sh <chrome-devtools|preview>

set -euo pipefail

if [ $# -ne 1 ] || ! [[ "$1" =~ ^(chrome-devtools|preview)$ ]]; then
    cat >&2 <<'EOF'
swap-browser.sh: usage: swap-browser.sh <chrome-devtools|preview>
  chrome-devtools  — Chrome DevTools MCP (mcp__chrome-devtools__*).
  preview          — Claude Preview MCP (mcp__Claude_Preview__preview_*).
EOF
    exit 1
fi
TARGET="$1"

cd "$(git rev-parse --show-toplevel)"
SKILL=".claude/skills/ui-validate/SKILL.md"
if [ ! -f "$SKILL" ]; then
    echo "swap-browser.sh: $SKILL not found." >&2
    exit 1
fi

# Detect current via the header marker.
CURRENT="$(sed -nE 's/^<!-- BROWSER_MCP:[[:space:]]*([a-z-]+)[[:space:]]*-->.*/\1/p' "$SKILL" | head -1)"
if [ -z "$CURRENT" ]; then
    echo "swap-browser.sh: no <!-- BROWSER_MCP: ... --> marker found in $SKILL." >&2
    exit 1
fi
if [ "$CURRENT" = "$TARGET" ]; then
    echo "swap-browser.sh: already on $TARGET; nothing to do." >&2
    exit 0
fi

# The substitution table. Each line is `chrome-devtools-name|preview-name`.
# These pairs are bidirectional — sed handles both directions from the same
# table. Tool names listed are the ones the skill body actually references.
read -r -d '' PAIRS <<'EOF' || true
mcp__chrome-devtools__new_page|mcp__Claude_Preview__preview_start
mcp__chrome-devtools__click|mcp__Claude_Preview__preview_click
mcp__chrome-devtools__fill|mcp__Claude_Preview__preview_fill
mcp__chrome-devtools__take_screenshot|mcp__Claude_Preview__preview_screenshot
mcp__chrome-devtools__take_snapshot|mcp__Claude_Preview__preview_snapshot
mcp__chrome-devtools__evaluate_script|mcp__Claude_Preview__preview_eval
mcp__chrome-devtools__list_console_messages|mcp__Claude_Preview__preview_console_logs
mcp__chrome-devtools__list_network_requests|mcp__Claude_Preview__preview_network
EOF

# Build a sed script that does ALL replacements in one pass. Use a tab as
# the sed delimiter so we don't have to escape the underscores / pipes in
# the tool names (the names contain no tabs).
SED_SCRIPT=""
while IFS='|' read -r cd_name pv_name; do
    [ -z "$cd_name" ] && continue
    if [ "$TARGET" = "preview" ]; then
        SED_SCRIPT="${SED_SCRIPT}s	${cd_name}	${pv_name}	g
"
    else
        SED_SCRIPT="${SED_SCRIPT}s	${pv_name}	${cd_name}	g
"
    fi
done <<<"$PAIRS"

# Header marker rewrite.
SED_SCRIPT="${SED_SCRIPT}s|<!-- BROWSER_MCP:[[:space:]]*[a-z-]\\{1,\\}[[:space:]]*-->|<!-- BROWSER_MCP: ${TARGET} -->|
"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

sed "$SED_SCRIPT" "$SKILL" >"$TMP"
mv "$TMP" "$SKILL"
trap - EXIT

# Verify the marker is now what we asked for.
NEW="$(sed -nE 's/^<!-- BROWSER_MCP:[[:space:]]*([a-z-]+)[[:space:]]*-->.*/\1/p' "$SKILL" | head -1)"
if [ "$NEW" != "$TARGET" ]; then
    echo "swap-browser.sh: post-swap marker is '$NEW', expected '$TARGET'." >&2
    exit 1
fi

echo "swap-browser.sh: $SKILL is now on $TARGET (was $CURRENT)."
