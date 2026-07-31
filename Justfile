# Launch the dev multiplexer with Zellij. Server tab auto-starts; the
# android/ios/ios-hybrid/playwright tabs are preloaded with their
# commands suspended — press Enter in the pane to start them.
serve:
    zellij --layout .zellij/layout.kdl

# Launch the dev multiplexer with process-compose. Server process auto-starts;
# android/ios/ios-hybrid/playwright are disabled by default — select
# one in the TUI and press F7 to start.
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
    scripts/with-dev-env.sh web scripts/dev-server-up.sh

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

# Drop the cached Nix dev-env captured by scripts/with-dev-env.sh. Use if a
# `nix store gc` or a weird env issue makes `just dev-up`/`lint`/`test`
# misbehave; the next run repopulates from a fresh `nix print-dev-env`.
env-clear:
    rm -rf .claude/runtime/nix-env-cache

# Restart THIS workspace's dev server cleanly. Use after adding a sqlx
# migration (the running server boots its migrations at startup) or when
# `dx serve` has wedged on a compile error and `dev-up` exits 2.
dev-bounce:
    scripts/with-dev-env.sh web scripts/dev-server-bounce.sh

# Ensure the public-domain test fixtures are present (one-time ~156 MB
# download from the fixtures-vN release asset, then an instant no-op).
# Runs outside the nix shell on purpose — curl + tar only. Auto-run by
# `just test`; Playwright's globalSetup fails loudly if they're missing.
fixtures:
    scripts/fetch-fixtures.sh

# Run the full unit/integration test matrix via cargo-nextest (the same runner
# CI uses, so local and CI results match; no doctests in the tree, which is all
# nextest can't run). A bare `--workspace` is a trap here — it silently skips
# the frontend rpc/page tests (they need --features server) — so run each crate
# explicitly. The frontend runs twice
# with different feature sets: `server` (rpc/page tests) and `mobile` (the
# `data::token_store` / `app_dirs` persistence tests) — the `web`/`mobile`/
# `server` impls are cfg-split, so each set exercises a different code path.
# The `omnibus-mobile` shell crate itself has no tests (lint covers it via
# `just lint`).
# Self-wraps in the slim nix shell so it works from a bare checkout too.
test: fixtures
    scripts/with-dev-env.sh default bash -ec '\
        cargo nextest run -p omnibus-db && \
        cargo nextest run -p omnibus && \
        cargo nextest run -p omnibus-frontend --features server && \
        cargo nextest run -p omnibus-frontend --features mobile && \
        cargo nextest run -p omnibus-shared'

# Line/region coverage over the same crate matrix as `just test`, via
# cargo-llvm-cov driving cargo-nextest (matches the CI `test` job). Each crate
# runs under `--no-report` to accumulate profraw, then a single `report` merges every
# feature-split run into one number — mirroring the `test` recipe so the
# coverage build exercises the identical code paths. `clean` first drops stale
# profiles from a previous run. Writes lcov.info (Codecov / CI) and prints the
# per-file summary. The wasm32 `web` feature is intentionally absent —
# llvm-cov can't instrument the wasm32 target, so coverage reflects the
# server + mobile-feature Rust only. Self-wraps in the slim nix shell.
coverage: fixtures
    scripts/with-dev-env.sh default bash -ec '\
        cargo llvm-cov clean --workspace && \
        cargo llvm-cov nextest --no-report -p omnibus-db && \
        cargo llvm-cov nextest --no-report -p omnibus && \
        cargo llvm-cov nextest --no-report -p omnibus-frontend --features server && \
        cargo llvm-cov nextest --no-report -p omnibus-frontend --features mobile && \
        cargo llvm-cov nextest --no-report -p omnibus-shared && \
        cargo llvm-cov report --lcov --output-path lcov.info && \
        cargo llvm-cov report'

# Same coverage run as `just coverage`, but emit a browsable HTML report
# (under $CARGO_TARGET_DIR/llvm-cov/html) instead of lcov — for local
# per-line drill-down. `--open` launches it in a browser.
coverage-html: fixtures
    scripts/with-dev-env.sh default bash -ec '\
        cargo llvm-cov clean --workspace && \
        cargo llvm-cov nextest --no-report -p omnibus-db && \
        cargo llvm-cov nextest --no-report -p omnibus && \
        cargo llvm-cov nextest --no-report -p omnibus-frontend --features server && \
        cargo llvm-cov nextest --no-report -p omnibus-frontend --features mobile && \
        cargo llvm-cov nextest --no-report -p omnibus-shared && \
        cargo llvm-cov report --html --open'

# Structural CSS lint — stylelint over frontend/assets, scoped to parse /
# structural errors only (unclosed rules, misplaced @import), not style.
# An unclosed `}` silently reparents later rules under CSS nesting, so a
# missing brace must fail the build. stylelint is Nix-pinned in slimPackages.
lint-css:
    scripts/with-dev-env.sh default stylelint 'frontend/assets/**/*.css'

# TypeScript lint + format-check + typecheck for the Playwright project.
# Biome (formatter + linter, config in ui_tests/playwright/biome.json) is the
# single source of truth, pinned as a pnpm devDependency; `tsc --noEmit` runs
# the TypeScript 7 compiler. Runs in `.#e2e` (which carries pnpm) and installs
# the frozen lockfile first so the toolchain matches CI. Separate from the
# slim `just lint` so daily cargo work doesn't pull the node toolchain.
lint-ts:
    scripts/with-dev-env.sh e2e bash -ec '\
        cd ui_tests/playwright && \
        pnpm install --frozen-lockfile && \
        pnpm exec biome check . && \
        pnpm exec tsc --noEmit'

# Format check + clippy, including the crate/feature combos a bare
# `cargo clippy` (default-members, default features) misses. `-D warnings` on
# every invocation and the wasm32 `frontend-web` variant mirror CI's clippy
# matrix (.github/workflows/rust.yml) so a clean local run can't go on to fail
# in CI. Depends on `lint-css` so the CSS structural guard rides the same gate
# as fmt/clippy.
lint: lint-css
    scripts/with-dev-env.sh default bash -ec '\
        cargo fmt --check && \
        cargo clippy --all-targets -- -D warnings && \
        cargo clippy -p omnibus-frontend --features server --all-targets -- -D warnings && \
        cargo clippy -p omnibus-mobile --all-targets -- -D warnings'
    scripts/with-dev-env.sh web cargo clippy -p omnibus-frontend --features web --all-targets --target wasm32-unknown-unknown -- -D warnings

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
        : "${CARGO_TARGET_DIR:="$HOME/.cache/cargo-target/$(basename "$(pwd)")"}"; \
        [ -n "$app" ] || app="$(find "$CARGO_TARGET_DIR/dx/omnibus-mobile" -type d -name "*.app" -path "*ios*" -print0 2>/dev/null | xargs -0 ls -dt 2>/dev/null | head -1)"; \
        scripts/apply-ios-icon.sh "$app" iphonesimulator'

# Build, brand, sign, and install Omnibus on a connected iPhone. Requires an
# Apple Developer membership + an "Apple Development" cert (Xcode ▸ Settings ▸
# Accounts) + the phone plugged in and trusted. dx handles the device build +
# provisioning + signing; the script injects the launcher icon and the App
# Transport Security exception (for the plain-http LAN server), then re-signs
# and installs. Pass a device name to disambiguate. See scripts/install-ios.sh.
install-ios device="":
    nix develop .#mobile --command scripts/install-ios.sh "{{device}}"

# --- Native SwiftUI app (omnibus-ios/, scheme `omnibus`). xcodebuild + simctl
# are system Xcode, so none of these wrap in a nix shell — the opposite: the
# `env -u LD -u CC -u CXX` prefixes strip the nix dev shell's toolchain exports
# (direnv loads them into every interactive shell), because xcodebuild adopts
# $LD as the linker driver and raw ld can't parse the clang-style args it then
# receives. Derived data lives under ~/.cache/omnibus-ios-derived/<worktree> —
# outside the repo, same philosophy as CARGO_TARGET_DIR.

# Compile check against a generic simulator destination — no booted device
# needed. Shares derived data with `ios-sim`, so a later sim run reuses it.
ios-build:
    env -u LD -u CC -u CXX xcodebuild build \
        -project omnibus-ios/omnibus.xcodeproj -scheme omnibus \
        -configuration Debug \
        -destination 'generic/platform=iOS Simulator' \
        -derivedDataPath "${OMNIBUS_IOS_DERIVED_DIR:-$HOME/.cache/omnibus-ios-derived/$(basename "$PWD")}"

# Unit tests (omnibusTests) via scripts/ios-test.sh — the exact invocation CI
# runs (.github/workflows/ios-tests.yml), so local and CI agree. Results land
# as .claude/runtime/ios-tests/<suite>.xcresult.
ios-test:
    env -u LD -u CC -u CXX scripts/ios-test.sh unit

# UI tests (omnibusUITests), same script/invocation as CI.
ios-test-ui:
    env -u LD -u CC -u CXX scripts/ios-test.sh ui

# Boot the newest iPhone simulator, build, install, and launch the native app.
# Prints this workspace's dev-server URL (from `just dev-up`'s env.sh) as a
# hint for the Connect screen. See scripts/ios-sim.sh.
ios-sim:
    scripts/ios-sim.sh
