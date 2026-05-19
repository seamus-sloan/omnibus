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

Override `PORT` (default `3000`) if you need a different port. Playwright targets `$PLAYWRIGHT_BASE_URL` (set by `scripts/dev-server-up.sh`); it falls back to `http://127.0.0.1:3000` when unset.

## `.env` for secret-bearing values

Non-secret defaults stay in the shellHook above. Anything with a secret — passwords, tokens, per-developer overrides — lives in a gitignored `.env` at the repo root. The shellHook sources `.env` **after** its own exports, so `.env` always wins on conflict.

- [`.env.example`](../../.env.example) is checked in and documents every supported var with example values.
- `.env` is gitignored. Copy from `.env.example` on first checkout.

Currently documented:

- `OMNIBUS_DEV_SEED_USER=username:password` — creates a named admin user on server boot if absent. Dev convenience for `ui-validate` and parallel agents; never set in production. Password must satisfy `db::auth` validation (≥10 chars, not in `COMMON_PASSWORDS`).

Optional thumbnail cache overrides (F1.2):
- `OMNIBUS_THUMBS_DIR` — where WebP thumbnails are cached (default `./thumbs`)
- `OMNIBUS_THUMBS_CAP_BYTES` — eviction cap in bytes (default 5 GiB)

If `ANDROID_HOME` / `ANDROID_NDK_HOME` come back empty, set them manually:

```bash
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
```

If a task requires a new system dependency, add it to [flake.nix](../../flake.nix) rather than documenting a manual install step.
