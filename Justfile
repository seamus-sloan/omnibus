# Launch the dev multiplexer with Zellij. Server tab auto-starts; android/ios/playwright
# tabs are preloaded with their commands suspended — press Enter in the pane to start them.
serve:
    zellij --layout .zellij/layout.kdl

# Launch the dev multiplexer with process-compose. Server process auto-starts;
# android/ios/playwright are disabled by default — select one in the TUI and press F7 to start.
serve-pc:
    process-compose up

# Idempotent dev-server bring-up. Probes /api/_health, walks the port from
# $PORT (default 3000) up to PORT+20 to find a free slot, starts `dx serve`
# in the background, and writes runtime files under .claude/runtime/. Use
# this when you want a server up but don't care which workspace owns :3000.
# `source .claude/runtime/env.sh` afterwards to pick up OMNIBUS_PORT and
# PLAYWRIGHT_BASE_URL for follow-on commands (preview, Playwright).
dev-up:
    scripts/dev-server-up.sh
