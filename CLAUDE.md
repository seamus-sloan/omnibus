# CLAUDE.md

Guidance for Claude Code when working in this repo. This file is an index — detailed rules and recipes live in [.claude/](.claude/).

Omnibus is a self-hosted ebook/audiobook library (see [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md)). The current counter app is a placeholder.

## Rules

Numbered rules in [.claude/rules/](.claude/rules/), applied in order. Follow them mechanically.

- [01-dev-environment.md](.claude/rules/01-dev-environment.md) — always work inside `nix develop`; env vars the shellHook sets.
- [02-error-handling.md](.claude/rules/02-error-handling.md) — `thiserror` for predictable failures, `anyhow` for unpredictable ones and handlers.
- [03-unit-testing.md](.claude/rules/03-unit-testing.md) — sibling `<mod>/tests.rs`, `test_support` per crate, happy + per-variant coverage.
- [04-playwright.md](.claude/rules/04-playwright.md) — full E2E conventions (selectors, fixtures, `expectMutation`, error paths).
- [05-rust-style.md](.claude/rules/05-rust-style.md) — Rust style guide: comments, function/file shape, errors, tests, mechanics. Long-form rationale in [docs/style-guide.md](docs/style-guide.md).
- [98-keep-skills-fresh.md](.claude/rules/98-keep-skills-fresh.md) — update skills when the code they reference changes.
- [99-end-of-session.md](.claude/rules/99-end-of-session.md) — end-of-session checklist (docs sync, fmt/clippy, coverage, line-count cap).

**Line-count cap:** every file in `CLAUDE.md` / `.claude/` stays under ~200 lines. Split by topic when it grows past that — enforced by rule 99.

## Skills

Auto-discoverable skills in [.claude/skills/](.claude/skills/) — Claude Code loads each `SKILL.md` automatically, and each is invokable via `/<name>` (e.g. `/ui-validate`).

- [add-backend-route](.claude/skills/add-backend-route/SKILL.md) — adding an Axum page or API endpoint end-to-end.
- [add-playwright-flow](.claude/skills/add-playwright-flow/SKILL.md) — adding a new E2E spec.
- [ui-validate](.claude/skills/ui-validate/SKILL.md) — bring up a port-walking dev server, log in as the seeded admin, drive the browser, poll `/api/_health` for rebuild signal.

### Browser tools

When you need to drive the running web app to verify a UI change, use **[`ui-validate`](.claude/skills/ui-validate/SKILL.md)**. It uses **Claude Preview** (`mcp__Claude_Preview__preview_*`) — each agent gets its own isolated headless Chromium, so parallel agents across workspaces never share a browser.

Do not use Chrome DevTools MCP or Claude in Chrome for routine agent verification — both share a single browser instance (one at a time), so they break down with parallel agents. Use `qa-browser` only when you explicitly want to watch the agent work in your real browser for a one-off session.

## Architecture

Cargo workspace with five crates:

- **`shared/`** (`omnibus-shared`) — serde types shared across every target (`Settings`, `ValueResponse`, `LibraryContents`, `LibrarySection`). No Dioxus / axum / sqlx deps.
- **`db/`** (`omnibus-db`) — server-side data layer: SQL migrations, SQLite pool init, the normalized query layer, and the indexing pipeline (scanner → ebook metadata extraction → atomic per-library upsert). Consumed by both `server/` (REST handlers) and `frontend/` (server-function bodies). Holds all sqlx / tokio / epub / anyhow dependencies on the server side.
- **`frontend/`** (`omnibus-frontend`) — Dioxus UI + server-function wire layer (`rpc.rs`). Feature-gated:
  - `web` — WASM client build (used by `server/` when `dx serve --platform web` builds it for WASM).
  - `mobile` — Native Dioxus build; uses `reqwest` against `/api/*` REST routes.
  - `server` — SSR/native build; pulls in `omnibus-db` and compiles server-function bodies. Name is hardcoded by the dioxus_fullstack_macro — can't be renamed.
- **`server/`** (`omnibus`) — **unified Dioxus fullstack binary**. Built twice by `dx serve`: once native (feature `server`) for the axum backend + SSR, once WASM (feature `web`) for the hydrated client. Hosts the hand-written `/api/*` REST router for mobile. Depends directly on `omnibus-db`.
- **`mobile/`** (`omnibus-mobile`) — thin Dioxus Native shell (~16 lines) that injects `ServerUrl` context and launches `omnibus_frontend::App`.

Default `cargo build` / `clippy` covers `server`, `shared`, `frontend` only. Mobile is excluded via workspace `default-members` because its `mobile` feature is mutually exclusive with `web`; build it explicitly: `cargo build -p omnibus-mobile`.

**Web request flow (fullstack):** browser → axum serves SSR'd HTML + WASM bundle → hydration → signal effects call Dioxus server functions (`#[get]`/`#[post]` in `frontend/src/rpc.rs`) at `/api/rpc/*` → same handlers execute server-side against the SQLite pool via an `axum::Extension<SqlitePool>` layer.

**Mobile data flow:** Dioxus signal/effect → `reqwest` call to `/api/*` (hand-written handlers in `server/src/backend.rs`) → signal update → re-render. Mobile deliberately does **not** use the `/api/rpc/*` server functions.

**Database:** schema ships as numbered SQL migrations under [db/migrations/](db/migrations/), embedded via `sqlx::migrate!` and run on pool init in `omnibus_db::init_db`. Applied versions are recorded in the `_sqlx_migrations` table. Add new migrations as `NNNN_description.sql` — never edit an applied file. All tests use `sqlite::memory:` for isolation; the migrator runs against them the same as production.

**Server URL (mobile):** hardcoded to `http://127.0.0.1:3000` in `mobile/src/main.rs` via `use_context_provider`. Will become a first-launch setup screen.

### shared/src/

```
lib.rs              — Settings, ValueResponse, LibraryContents, LibrarySection, EbookMetadata, EbookLibrary, AuthorDetail, SeriesDetail, TagWeight, ViewPrefs, ViewFilters, MetadataOverrides, Contributor, Identifier, PaletteResults + palette hit types (F1.5); re-exports image_format::detect_image_format
image_format.rs     — magic-byte image-format sniff (detect_image_format); pure &[u8] inspection, single source of truth for both server::backend and frontend::rpc upload paths
```

### db/src/

```
lib.rs              — re-exports queries::* and thumbs::{ThumbSize, thumb_path_for, thumbs_dir, ThumbError}; pub mod auth/author_photos/ebook/indexer/library_layout/queries/scanner/thumbs/worker
queries.rs          — pool init, schema, query layer (list_books, settings, covers, taxonomy, metadata_overrides CRUD + merge, author_photos CRUD…)
progress.rs         — F2.1 server-authoritative reading/listening position + batched session reports. `upsert_progress` (last-write-wins on `(user, book, format)`) / `get_progress` / `record_session`. Backs `/api/progress*` REST (mobile) and `/api/rpc/progress*` server functions (web). Schema migrations/0013_reading_progress.sql also lays out `bookmarks` / `reading_sessions` / `listening_sessions` for F2.3 + stats + bookmarks UI to ship cheaply later.
auth.rs             — F0.3 auth data layer (users/devices/sessions): Argon2id hash + verify with PHC rotation, password-policy validation, race-free first-user-admin (BEGIN IMMEDIATE), per-account lockout, SHA-256-hashed session tokens with absolute + idle expiry, `promote_to_admin` recovery hook. Pure SQL + hashing — cookies/axum live in `server/src/auth/`. `validate_session(pool, authorization, cookie_header) -> Result<(User,Session), SessionAuthError>` is the single consolidated cookie/bearer → live-session surface (axum-free, pure-string headers) that both HTTP auth extractors (`server::auth::extractor` + `omnibus_frontend::rpc`) delegate to so they can't drift. Schema: migrations/0004_auth.sql
scanner.rs          — library directory scanning
ebook.rs            — EPUB OPF metadata + cover extraction; sidecar-first cover with opt-in materialization (ScanOptions)
audiobook.rs        — F2.3 audiobook metadata extraction via `lofty` (m4b/m4a/mp3): title/artist/album/duration + embedded artwork. Phase A.5 `group_into_books` groups per-file stat entries into one `AudiobookGroup` per book (folder-of-mp3s = one group). Phase B `parse_groups` reads ID3 tags for every part, sorts by (track, filename), and emits `IndexedAudiobook` rows for `sync::sync_audiobooks`. Submodule `audiobook::codec` (#339) hosts `PlaybackMode`, `classify_filenames`, `mime_for_filename` — the gate that decides direct-play vs HLS for the new manifest endpoint.
hls.rs              — F2.3 HLS transcode cache (fallback path after #339): `resolve_audiobook` (only admits books whose `book_files.format` is M4B/M4A/MP3), `get_parts`, `build_manifest` (VOD m3u8 from stored durations), `transcode_book` (ffmpeg concat→AAC-LC 64 kbps mono, 10 s segments), `evict_if_over_cap` (FIFO-by-mtime). Only invoked for non-natively-playable parts (a flac mixed into an mp3 folder, etc.); m4b/m4a/mp3 direct-play via `/api/audiobooks/:uuid/parts/:ordinal`. Reads `OMNIBUS_DATA_DIR`, `OMNIBUS_HLS_CAP_BYTES`, `OMNIBUS_FFMPEG_PATH`, `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS` — see `.env.example` for defaults.
thumbs.rs           — F1.2 thumbnail pipeline: decode cover → resize to Sm/Md/Lg (2:3) → atomic per-(book,size) WebP writes under OMNIBUS_THUMBS_DIR; FIFO-by-mtime eviction once total cache exceeds OMNIBUS_THUMBS_CAP_BYTES (default 5 GiB). Backs backend.rs's cover-to-WebP serving path
indexer.rs          — scan → DB indexing, staleness checks (is_stale + reindex); reindex opts into materialize_sidecars. F2.3 `reindex_audiobooks` uses Phase A.5 group_into_books + Phase B parse_groups + sync_audiobooks (writes book_file_parts rows).
library_layout.rs   — F0.6 canonical layout: slugify, canonical_path, sidecar_cover_for, allocate_canonical_path (collision suffix)
worker/             — single-process Worker primitive: scan_sem (Scan/ScanAudiobooks) + hls_sem (HlsTranscode) semaphores + per-resource keyed mutex; Task::HlsTranscode dispatches into hls::transcode_book; WorkerConfig::hls_concurrency defaults to max(1, num_cpus/2). F1.11 ResolveAuthorPhoto dispatches into author_photos::resolve. Split into types (Task/Worker/config), queue (post/await_completion + RAII map-cleanup guards), exec (run dispatch loop), handlers (per-task-kind execute arms), progress (snapshot + retention + report_progress), tests
author_photos/      — F1.11 author profile photo pipeline split into focused submodules: `cascade.rs` (manual → Open Library → letter resolution; `OpenLibraryConfig` for wiremock injection), `remote.rs` (`fetch_remote_image` + SSRF guard — `RemoteImageConfig`, `FetchRemoteImageError`; resolves host, blocks loopback/RFC1918/link-local/AWS IMDS/IPv6 ULA before any TCP connect, pins `reqwest` to validated addresses against DNS rebinding; `allow_private_addresses` is test-only), `shared.rs` (process-wide `reqwest::Client` singleton + `default_user_agent`), `tests.rs` (wiremock-backed cascade + SSRF guard unit tests). Public API re-exported from `mod.rs`.
author_photos_data.rs — Row CRUD for the `author_photos` table (`get_author_photo`, `author_photo_status`, `upsert_author_photo`, `delete_author_photo`) plus `delete_author` admin primitive. `AuthorPhotoSource` enum. `AuthorPhotosDataError` wraps `sqlx::Error` per boundary rule.
migrations/         — numbered SQL migrations embedded via sqlx::migrate!
```

### frontend/src/

```
lib.rs              — Route, App, styles, ScreenLayout (feature-gated); also owns the App-wide `CurrentUser` context (`use_current_user`) — a single `/api/auth/me` fetch on boot fills it, components gate `is_admin` off the cached signal instead of refetching per mount, and a `web_auth_state` subscription refills on login / clears on 401. Web/SSR only — mobile uses bearer tokens.
data.rs             — Module root for the feature-gated data transport (mobile=reqwest, web/server=rpc); owns the `DataError` thiserror enum (Network/Http/Decode/Unauthorized/Other), the mobile transport helpers (`ServerUrl`, `http_client`, `with_bearer`, `note_status`, `drain_error`, `client_kind`), `token_store` (mobile bearer-token store), `web_auth_state` (web 401 channel), and `note_server_fn_err` (web server-fn 401 mapper). Per-domain wrappers live under data/ and are re-exported here.
data/               — Per-domain wrappers split out from data.rs: `books.rs` (settings/library/ebooks/search/overrides/worker), `authors.rs` (author CRUD + photo upload/scan/delete), `series.rs`, `tags.rs`, `progress.rs` (F2.1 reading/listening sync), `auth.rs` (login/register/logout/me + mobile bearer flow). Every wrapper keeps its existing `#[cfg(feature = …)]` gates and platform-agnostic signature.
view_prefs.rs       — F1.3 per-library ViewPrefs/ViewFilters persistence for the landing page (sort/filter/facet state); web → localStorage keyed by library path, mobile → in-memory map (resets on cold launch), server (SSR) → defaults so first-hydration markup matches. Shape lives in omnibus-shared for a future server-backed endpoint
reader_progress.rs  — F2.2 reading-position **cache-aside layer** for the F2.1 progress-sync endpoint: web → localStorage keyed by uuid, mobile → in-memory map, SSR → no-op. The reader reads this sync for instant first-paint then reconciles against `GET /api/progress/{uuid}` (server wins on success), and writes here AND fire-and-forget POSTs to `/api/rpc/progress` on every page turn.
audiobook_progress.rs — F2.3 listening-position cache-aside layer, audio analogue of `reader_progress`. Stores seconds (float) keyed by uuid (`omn.listening::{uuid}`), plus a single device-wide playback-rate preference (`omn.listening.rate`). Same web/mobile/SSR split; the listen page reads the local value for instant first-paint, reconciles against `GET /api/progress/{uuid}?format=audio`, and writes locally + fire-and-forget POSTs to `/api/rpc/progress` on every 5 s of playback plus on pause.
rpc.rs              — #[get]/#[post] server functions (mounted by dioxus::server::router); server bodies call into omnibus_db
pages/{landing,settings,book_detail,metadata_edit,reader,listen,auth,author,authors_index,series,series_index,tag_cloud,search}.rs  — auth.rs hosts LoginPage + RegisterPage. reader.rs is the F2.2 immersive full-screen EPUB reader at /read/:uuid (no app nav; loads the vendored epub.js runtime, drives window.OmnibusReader via dioxus::document::eval, persists position via reader_progress; web-only interop, chrome compiles on every target). Vendored reader runtime lives at frontend/assets/vendor/{jszip.min.js,epub.min.js,epub-reader-glue.js} and is loaded only on the reader page. listen.rs is the F2.3 immersive audiobook player at /listen/:uuid — plain HTML5 `<audio>` element (no vendored library), inline `dioxus::document::eval` bootstrap exposes `window.OmnibusAudio` (play/pause/seek/skip/setRate plus `initDirect`/`initHls`). Boots by fetching `/api/audiobooks/:uuid/manifest` and branching: `mode:"direct"` chains Range-served per-part URLs (m4b/m4a/mp3, instant) with `ended`-driven advance; `mode:"hls"` falls back to status-polling + hls.js attach. Surfaces `state:"failed"` from the /status endpoint as a real failure overlay (Bug 4 of #338). Persists position via audiobook_progress + the F2.1 endpoint. Landing is the primary Atrium consumer (cover grid + power-user table) and the source of format-faceted filtering (ViewFilters.formats in shared). metadata_edit.rs is the F5.1 single-book edit form at /books/:id/edit. Discovery pages (author, series, tag_cloud) are F1.8. authors_index.rs and series_index.rs are the F1.12 `/authors` and `/series` browse-all index surfaces. search.rs is the full-page /search/:query results view that the F1.5 palette routes into when the user presses Enter without arrow-key navigation, or when a Tag row is selected.
components/{top_nav,bottom_nav}.rs  — feature = web / mobile respectively
components/atrium.rs  — F1.7 design-system primitives (AtriumRoot, Cover); the app-wide `Theme` enum (Dark/Light/Sepia) + init_theme/persist_theme; CSS at frontend/assets/atrium.css. Live theme switcher is UmThemeSeg in user_menu.rs.
components/search_palette.rs — F1.5 command-palette search overlay (⌘K trigger, grouped FTS5 results, keyboard nav); web-only (not(mobile))
components/format_switcher.rs — F1.4 per-format CTA rows on the book detail page (Read for EPUB → F2.2 reader; Listen for M4B/M4A/MP3 → F2.3 player at /listen/:uuid on web, disabled on mobile; Send-to-Kindle still stubbed); the UI contract for the F0.1 books/book_files split
components/author_photo_edit.rs — F1.11 admin photo-edit overlay (web-only): hover-revealed pencil button over the avatar opening a modal with paste-URL / file-upload (→ PUT /api/authors/:id/photo) / re-scan-Open-Library actions. Wraps avatars on the author detail and authors-index pages
components/chip_editor.rs — F5.9-lite reusable add/remove chip editor with ≤5-row prefix-match suggestion dropdown (↓/↑ navigates, Enter commits, Esc clears). Renders chips + input + dropdown as siblings of the parent flex row so the existing `me-chip-row` / `me-tag-chips` layouts keep their flow. Consumed by the F5.1 metadata edit page (authors + tags) — the consumer passes a `Vec<String>` candidate pool (typically derived from `data::list_authors` + `data::get_tag_cloud`); empty pool degrades to plain free-text entry.
components/auth/{mod,shell,banner,field,strength}.rs — F1.6 auth-UI primitives (purely presentational, SSR/WASM identical for hydration) used by pages/auth.rs: AuthShell split-pane wrapper, Banner callout (err/warn/info/ok), Field label+input+hint/error/success slots, StrengthMeter four-segment password bar
components/user_menu.rs — avatar trigger + dropdown panel in TopNav; hosts the real Settings link, Sign out action, and Dark/Light theme segmented control (Sepia stubbed). Most rows are forward-looking stubs (disabled <a>) until the underlying features (Profile, Journal, Highlights, Shelves, Goals, Notifications, Switch user) ship. Web-only (not(mobile)).
```

### server/src/

```
main.rs             — dioxus::launch (WASM) / dioxus::serve (native); mounts auth_router + rate-limit + origin-check + security_headers
lib.rs              — re-exports backend + auth + rate_limit + security_headers under `server` feature for tests
backend.rs          — /api/* REST router (mobile-facing) + integration tests; `/api/search/*` sub-router carries its own per-IP rate-limit layer. `GET /api/ebooks/{uuid}/file` (F2.2) streams the raw EPUB bytes (application/epub+zip, cookie-gated) for the reader; resolves the on-disk path via `db::book_file_path`. F2.1 sub-module `backend/progress.rs` mounts `POST /api/progress` (discriminated `{format: "epub"|"audio"}` payload, last-write-wins, 404 on unknown book), `GET /api/progress/{uuid}?format=…`, and `POST /api/progress/sessions` (batched `SessionReport` rows).
backend/audiobooks.rs — F2.3 audiobook playback routes: `GET /api/audiobooks/{uuid}/manifest` returns `{mode:"direct", parts:[…]}` (m4b/m4a/mp3 — instant playback) or `{mode:"hls", playlist_url}` (mixed folders containing flac/ac3/… — falls back to transcode) per `db::audiobook::codec::classify_filenames`. `GET /api/audiobooks/{uuid}/parts/{ordinal}` Range-serves source files via `tower_http::ServeFile`; mirrors the same classifier gate so HLS-classified books 404 instead of leaking source bytes around the manifest contract. Legacy HLS routes (`/playlist.m3u8`, `/segments/{seg}`, `/status`) retained as the fallback path. See #339.
rate_limit.rs       — reusable in-memory per-IP fixed-window counter + `rate_limit_by_ip` axum middleware; `rate_limit_paths` is a path-prefix wrapper used for both `/api/rpc/search-palette` and the auth router (allow-list = login/register/logout, so `/api/auth/me` is deliberately exempt from the 10/60s bucket)
security_headers.rs — #277 global HTTP security response headers: CSP (Dioxus-WASM-compatible), X-Frame-Options DENY, Referrer-Policy strict-origin-when-cross-origin, X-Content-Type-Options nosniff applied unconditionally; HSTS (1y + includeSubDomains) gated on OMNIBUS_SECURE_COOKIES so plain-HTTP dev origins don't advertise the wrong policy
auth/mod.rs         — /api/auth/{register,login,logout,me} + AuthUser/AdminUser extractors + CSRF origin-check
auth/gate.rs        — top-level middleware gating /api/* (pass-through for /api/auth/*)
auth/strategy.rs    — AuthStrategy trait + PasswordStrategy (OIDC/WebAuthn fit the same shape)
auth/boot.rs        — OMNIBUS_INITIAL_ADMIN recovery hook (promotes named user to admin)
```

### ui_tests/playwright/

```
tests/
  flows/            — one *.spec.ts per user flow
  utils/            — cross-flow helpers (nav, api mutation assertions)
  fixtures/         — extended `test` / `expect` exports
```

### mobile/src/

```
main.rs             — dioxus::launch, ServerUrl context, hydrates bearer token from disk, wraps omnibus_frontend::App
```

Mobile auth: bearer-token login flow lives in `frontend/src/data.rs` under
`feature = "mobile"`. `data::mobile_login` / `mobile_register` POST to
`/api/auth/{login,register}` with `client_kind: ios|android|bearer` so the
server returns a bearer token in the JSON body. `data::token_store` keeps
the token in a process-local `OnceLock<RwLock<...>>` and — **only in
debug builds** (`cfg!(debug_assertions)`) — persists it to
`$HOME/.omnibus-token`. Release builds keep the token in memory only and
require re-login on every cold start. UI components subscribe to
`data::token_store::subscribe()` (a `tokio::sync::watch` receiver) so a
401-driven `token_store::clear()` reactively redirects to `/login`.
**TODO**: replace the disk-persistence stub with iOS Keychain / Android
Keystore and flip persistence on unconditionally — see the module docs.

## Quick commands

```bash
# Multiplexed dev stack (server + ios + android + playwright panes)
just serve                                                  # Zellij
just serve-pc                                               # process-compose

# Idempotent dev-server bring-up — port-walks from $PORT (default 3000)
# up to PORT+20, reuses an existing omnibus server if found, seeds the
# admin user from $OMNIBUS_DEV_SEED_USER, writes .claude/runtime/{port,env.sh}.
# Used by the `ui-validate` skill; safe to re-run.
just dev-up
source .claude/runtime/env.sh                               # picks up OMNIBUS_PORT + PLAYWRIGHT_BASE_URL

# Fullstack dev (serves SSR + WASM hydration at http://localhost:8080 by default)
dx serve --platform web -p omnibus

# Server only (native backend, no WASM bundle)
cargo run -p omnibus                                        # start at http://0.0.0.0:3000
cargo test -p omnibus                                       # /api/* REST integration tests
cargo test -p omnibus-db                                    # db + ebook + scanner tests
cargo clippy                                                # lint default-members crates
cargo fmt                                                   # format all crates

# Playwright E2E (server must be running; baseURL = $PLAYWRIGHT_BASE_URL or :3000)
cd ui_tests/playwright && npm install                       # first time
cd ui_tests/playwright && npx playwright test               # run all

# Mobile
cargo build -p omnibus-mobile
xcrun simctl boot "iPhone 17" 2>/dev/null; dx serve --platform ios --package omnibus-mobile
dx serve --platform android --package omnibus-mobile
adb reverse tcp:3000 tcp:3000                               # after Android emulator boots
```

## Project direction

See [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md) for the phased roadmap (foundations, browse/discovery, reading/listening, personalization, device sync, admin, mobile).
