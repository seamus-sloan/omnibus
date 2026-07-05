# 01 — Dev environment

All system dependencies (Rust toolchain, SQLite, pkg-config, OpenSSL, Node.js, Android SDK/NDK, JDK) are provided by Nix. **Always** work inside the dev shell:

```bash
nix develop --command zsh   # preferred — keeps your shell prompt intact
nix develop                 # also works; spawns a bash subshell
```

## Shells

The flake exposes five purpose-built shells so daily cargo work doesn't pay for Playwright + mobile + audit tooling. Pick the smallest one that has what you need:

| Shell | Headline tools | When to use |
|---|---|---|
| `default` (slim) | rust core + sqlite + openssl + just + zellij + process-compose + stylelint | Daily `cargo test`/`clippy`/`fmt`, editor, rust-analyzer, `just lint-css` — this is what direnv auto-loads via `.envrc` |
| `web` | default + dioxus-cli + matched `wasm-bindgen` + node | `dx serve --platform web`, `just dev-up`, anything that bundles the WASM client |
| `e2e` | web + Playwright Chromium bundle | `npx playwright test`, the `playwright` pane in the multiplexer, CI's E2E job |
| `mobile` | default + dioxus-cli (`dx`) + Android + iOS rust-std targets + JDK 21 + Xcode/Android SDK auto-detect (+ GTK 3 / WebKitGTK on Linux) | `dx serve --platform ios`/`android`, `cargo build -p omnibus-mobile`; CI's `cargo clippy (mobile)` host-target lint |
| `audit` | default + cargo-audit + cargo-deny | Local `cargo audit` / `cargo deny`, mirrors CI's security job |

One-shot pattern for any non-default shell:

```bash
nix develop .#web    --command dx serve --platform web -p omnibus
nix develop .#audit  --command cargo deny check advisories sources bans
nix develop .#mobile                  # interactive shell with Android NDK + iOS targets
```

`.envrc` resolves `use flake` to `default`, so the editor stays on the slim shell at all times. `just serve` works from default because zellij + process-compose live there; each multiplexer pane internally wraps its command in the right `.#shell` (server → `.#web`, mobile → `.#mobile`, playwright → `.#e2e`), so only the panes you actually start realize their extras. `just dev-up` and `just dev-bounce` self-wrap in `.#web`, so they work straight from default too.

## CSS structural lint

`stylelint` lives in the slim shell so `just lint` (which runs `just lint-css`) and the `css-lint.yml` CI job both guard `frontend/assets/**.css` against structural errors — chiefly an unclosed `}`, which under CSS nesting silently reparents every following rule as a descendant of the unclosed selector and breaks layouts the Rust/web build never exercises. The ruleset ([`.stylelintrc.json`](../../.stylelintrc.json)) is parse/structural-only, so it errors on broken CSS but never nags about pre-existing style. Run `just lint-css` to check just the CSS.

## Common environment

Every shell sets:

- `DATABASE_URL=sqlite://omnibus.db?mode=rwc`
- `CARGO_TARGET_DIR=$HOME/.cache/cargo-target/<worktree-root-name>` — keeps `target/` outside the repo so flake evaluations don't snapshot multi-GB build artifacts into `/nix/store` on every direnv reload. The worktree root is resolved via `git rev-parse --show-toplevel` (so `nix develop` from a subdir picks the same dir), and the basename keeps it per-worktree to avoid races between parallel worktrees.
- `OMNIBUS_PUBLIC_ORIGIN=http://localhost:$PORT` — comma-separated allowlist consumed by `auth::origin_check`. Required for `dx serve --fullstack`: its HTTP proxy rewrites `Host` to the upstream backend's loopback address without setting `X-Forwarded-Host`, so without an allowlist every cookie-authed POST 403s. Override in production deployments behind a reverse proxy.
- `RUSTC_WRAPPER=sccache` + `CARGO_INCREMENTAL=0` + `SCCACHE_DIR=$HOME/.cache/sccache` + `SCCACHE_CACHE_SIZE=20G` — **local dev shells only** (nested inside the same `CARGO_TARGET_DIR`-unset guard, so CI — which pins `CARGO_TARGET_DIR=./target` and caches `target/` via `Swatinem/rust-cache` — is excluded automatically). Routes rustc through [sccache](https://github.com/mozilla/sccache) so identical crate builds are shared across every worktree via one cache at `$HOME/.cache/sccache` (build once, reuse everywhere). `SCCACHE_DIR` is pinned explicitly because sccache's default location is platform-specific (`~/.cache/sccache` on Linux, `~/Library/Caches/Mozilla.sccache` on macOS) — pinning keeps the shared cache in one place across worktrees and platforms. Incremental compilation is turned **off** deliberately: its per-crate codegen caches are what balloon each worktree's `target/` to tens of GB, and sccache can't cache incremental units — the two are mutually exclusive, so incremental-off is what makes sccache effective. Export `OMNIBUS_NO_SCCACHE=1` to opt out (restores incremental rebuilds — faster inner loop on the crate you're actively editing, at the cost of the `target/` bloat). Override `SCCACHE_CACHE_SIZE` to resize the shared cache (default 20G).

Shell-specific additions:

- `e2e` only — `PLAYWRIGHT_BROWSERS_PATH` → Nix-provided Chromium (don't run `npx playwright install`)
- `mobile` only — `ANDROID_HOME`, `ANDROID_NDK_HOME` (auto-detected from standard Android Studio install paths), plus the Xcode `DEVELOPER_DIR`/`SDKROOT`/`PATH` shim on macOS

Override `PORT` (default `3000`) if you need a different port. Playwright targets `$PLAYWRIGHT_BASE_URL` (set by `scripts/dev-server-up.sh`); it falls back to `http://127.0.0.1:3000` when unset.

## `.env` for secret-bearing values

Non-secret defaults stay in the shellHook above. Anything with a secret — passwords, tokens, per-developer overrides — lives in a gitignored `.env` at the repo root. The shellHook sources `.env` **after** its own exports, so `.env` always wins on conflict.

- [`.env.example`](../../.env.example) is the **canonical, complete reference** — every supported var is documented there with example values and warnings. Keep it and this rule in sync (rule 99 step 3).
- `.env` is gitignored. Copy from `.env.example` on first checkout.

Key vars (see `.env.example` for the full annotated list):

- `OMNIBUS_DEV_SEED_USER=username:password` — creates a named admin user on server boot if absent. Dev convenience for `ui-validate` and parallel agents; never set in production. Password must satisfy `db::auth` validation (≥10 chars, not in `COMMON_PASSWORDS`).
- `OMNIBUS_SECURE_COOKIES=0` — disables the `Secure` flag on session cookies. The default is `true` (secure-by-default). Browsers treat `http://localhost` as a secure context, so the Nix dev shell does not need this override; only set `0` when serving plain `http://` from a non-localhost origin (e.g. an IP-based dev server on a LAN). Never set in production.
- `EBOOK_LIBRARY_PATH` / `AUDIOBOOK_LIBRARY_PATH` — pre-seed library paths via the `seed_settings_from_env` boot hook (runs every boot; overwrites the settings row with BOTH values, so set both together or neither). For CI / Docker / dev-up; leave unset in production.
- `HARDCOVER_API_KEY` — account-level Hardcover Bearer token for F3.3 "Readers also enjoyed" suggestions. Unlike the library paths, the **Settings page is the source of truth**: the `seed_hardcover_key_from_env` boot hook seeds this value only when none is saved (settings wins thereafter), and `effective_hardcover_api_key` falls back to it when settings is empty. Server-wide, never per-user; leave unset to keep suggestions disabled.
- `SMTP_HOST` / `SMTP_PORT` / `SMTP_SECURITY` / `SMTP_USERNAME` / `SMTP_PASSWORD` / `SMTP_FROM_EMAIL` — F4.3 Send-to-Kindle SMTP relay. Same source-of-truth model as the Hardcover key: `seed_smtp_from_env` seeds the config only when no host is saved (Settings page wins thereafter), and `effective_smtp_config` falls back to the env vars when settings is empty. `SMTP_HOST` + `SMTP_FROM_EMAIL` are required; port defaults to 587 and security to `starttls` (`tls` for implicit TLS on 465). Server-wide; leave unset to keep Send-to-Kindle disabled. (A user's own `kindle_email` lives per-account on the `/account` page, not here.)

**Security-sensitive (never set casually in production):**

- `OMNIBUS_INITIAL_ADMIN=username` — promotes the named user to admin on **every** boot while set. One-time account recovery only — UNSET IMMEDIATELY AFTER USE.
- `OMNIBUS_TRUST_FORWARDED_FOR=1` — trust the client `X-Forwarded-For` as the per-IP rate-limit key. MUST NOT be set unless a trusted reverse proxy strips inbound `X-Forwarded-For`; on a directly-exposed deployment it lets any client spoof a fresh bucket and bypass the login throttle (credential stuffing).

Optional storage overrides:
- `OMNIBUS_COVERS_DIR` — where cover image files are stored (default `./covers`). Set to an absolute path on real deployments so covers don't land next to the binary and disappear on redeploy.
- `OMNIBUS_THUMBS_DIR` — where WebP thumbnails are cached (default `./thumbs`)
- `OMNIBUS_THUMBS_CAP_BYTES` — eviction cap in bytes (default 5 GiB)
- `OMNIBUS_MAX_UPLOAD_BYTES` — max accepted size for an "add your own books" upload, as both the upload routes' body limit and a per-file check (default 1 GiB)

HLS audiobook transcode cache (read by `db::hls`):
- `OMNIBUS_DATA_DIR` — base data dir; HLS segments live under `$OMNIBUS_DATA_DIR/hls/` (default `./data`)
- `OMNIBUS_HLS_CAP_BYTES` — HLS cache eviction cap in bytes (default 5 GiB)
- `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS` — ffmpeg watchdog per book in seconds (default 1800)
- `OMNIBUS_FFMPEG_PATH` — explicit ffmpeg path; otherwise ffmpeg must be on `$PATH`

Kobo KEPUB conversion (read by `db::kepub`, for the "Send to Kobo" download):
- `OMNIBUS_KEPUBIFY_PATH` — explicit kepubify path; otherwise kepubify must be on `$PATH`. Cache lives under `$OMNIBUS_DATA_DIR/kepub/`. Absent → download falls back to plain EPUB with a one-time startup warning. Bundled in the `.#web` shell and the release image.

If `ANDROID_HOME` / `ANDROID_NDK_HOME` come back empty inside `nix develop .#mobile`, set them manually:

```bash
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
```

If a task requires a new system dependency, add it to [flake.nix](../../flake.nix) rather than documenting a manual install step.
