# 01 — Dev environment

All system dependencies (Rust toolchain, SQLite, pkg-config, OpenSSL, Node.js, Android SDK/NDK, JDK) are provided by Nix. **Always** work inside the dev shell:

```bash
nix develop --command zsh   # preferred — keeps your shell prompt intact
nix develop                 # also works; spawns a bash subshell
```

The shell hook sets:

- `DATABASE_URL=sqlite://omnibus.db?mode=rwc`
- `CARGO_TARGET_DIR=$HOME/.cache/cargo-target/<worktree-root-name>` — keeps `target/` outside the repo so flake evaluations don't snapshot multi-GB build artifacts into `/nix/store` on every direnv reload. The worktree root is resolved via `git rev-parse --show-toplevel` (so `nix develop` from a subdir picks the same dir), and the basename keeps it per-worktree to avoid races between parallel jj workspaces.
- `PLAYWRIGHT_BROWSERS_PATH` → Nix-provided Chromium (don't run `npx playwright install`)
- `OMNIBUS_PUBLIC_ORIGIN=http://localhost:$PORT` — comma-separated allowlist consumed by `auth::origin_check`. Required for `dx serve --fullstack`: its HTTP proxy rewrites `Host` to the upstream backend's loopback address without setting `X-Forwarded-Host`, so without an allowlist every cookie-authed POST 403s. Override in production deployments behind a reverse proxy.
- `ANDROID_HOME` and `ANDROID_NDK_HOME` (auto-detected from standard Android Studio install paths)

Override `PORT` (default `3000`) if you need a different port.

If `ANDROID_HOME` / `ANDROID_NDK_HOME` come back empty, set them manually:

```bash
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
```

If a task requires a new system dependency, add it to [flake.nix](../../flake.nix) rather than documenting a manual install step.
