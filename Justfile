# Launch the dev multiplexer with Zellij. Server tab auto-starts; android/ios/playwright
# tabs are preloaded with their commands suspended — press Enter in the pane to start them.
serve:
    zellij --layout .zellij/layout.kdl

# Launch the dev multiplexer with process-compose. Server process auto-starts;
# android/ios/playwright are disabled by default — select one in the TUI and press F7 to start.
serve-pc:
    process-compose up

# Idempotent dev-server ensure-running. Probes /api/_health for THIS
# workspace's server (identity-checked via repo_root); reuses if healthy,
# starts a fresh daemonized `dx serve` if missing. Port-walks $PORT..$PORT+9
# (default $PORT = 3000 in the `default` workspace; per-workspace stride
# of 10 set in flake.nix). Emits one parseable line on stdout:
#     OMNIBUS_DEV_READY port=<n> repo_root=<path> build_id=<n> action=<reuse|started> pid=<n>
# Use `source .claude/runtime/env.sh` afterwards to pick up OMNIBUS_PORT
# and PLAYWRIGHT_BASE_URL for follow-on commands (Playwright, preview).
dev-up:
    scripts/dev-server-up.sh

# Peek at THIS workspace's dev server without mutating anything.
# Exit 0 + emit OMNIBUS_DEV_READY when healthy; exit 1 if no server is
# running for this workspace; exit 2 if a server is bound but unhealthy
# (likely a wedged dx serve build — try `just dev-bounce`).
dev-status:
    scripts/dev-server-status.sh

# Stop THIS workspace's dev server. Identity-checked: refuses to kill a
# PID whose /api/_health reports a different repo_root. Use after a long
# session or when you want to free the port entirely.
dev-down:
    scripts/dev-server-down.sh

# Restart THIS workspace's dev server cleanly. Use after adding a sqlx
# migration (the running server boots its migrations at startup) or when
# `dx serve` has wedged on a compile error and `dev-up` exits 2.
dev-bounce:
    scripts/dev-server-bounce.sh
