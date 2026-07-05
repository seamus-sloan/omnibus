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
    nix develop .#web --command scripts/dev-server-up.sh

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
    nix develop .#web --command scripts/dev-server-bounce.sh

# Run the full unit/integration test matrix. `cargo test --workspace` is a
# trap here — it silently skips the frontend rpc/page tests (they need
# --features server) — so run each crate explicitly. Mobile is not listed
# because it has no tests of its own (lint covers it via `just lint`).
# Self-wraps in the slim nix shell so it works from a bare checkout too.
test:
    nix develop --command bash -ec '\
        cargo test -p omnibus-db && \
        cargo test -p omnibus && \
        cargo test -p omnibus-frontend --features server && \
        cargo test -p omnibus-shared'

# Structural CSS lint — stylelint over frontend/assets, scoped to parse /
# structural errors only (unclosed rules, misplaced @import), not style.
# An unclosed `}` silently reparents later rules under CSS nesting, so a
# missing brace must fail the build. stylelint is Nix-pinned in slimPackages.
lint-css:
    nix develop --command stylelint 'frontend/assets/**/*.css'

# Format check + clippy, including the crate/feature combos a bare
# `cargo clippy` (default-members, default features) misses. Depends on
# `lint-css` so the CSS structural guard rides the same gate as fmt/clippy.
lint: lint-css
    nix develop --command bash -ec '\
        cargo fmt --check && \
        cargo clippy --all-targets && \
        cargo clippy -p omnibus-frontend --features server --all-targets && \
        cargo clippy -p omnibus-mobile --all-targets'

# Lint then test — the pre-push gate referenced by rule 99.
check: lint test

# Inject the Omnibus launcher icon into a built iOS `.app`. dx 0.7 installs no
# iOS app icon and `[bundle] icon` only feeds the desktop bundlers
# (DioxusLabs/dioxus#3685), so run this after `dx build --platform ios` and
# before `simctl install`. Defaults to the debug simulator build; pass a path
# to target another. Re-runnable/idempotent — see scripts/apply-ios-icon.sh.
ios-icon app="":
    nix develop .#mobile --command bash -ec '\
        app="{{app}}"; \
        [ -n "$app" ] || app="$CARGO_TARGET_DIR/dx/omnibus-mobile/debug/ios/OmnibusMobile.app"; \
        scripts/apply-ios-icon.sh "$app" iphonesimulator'
