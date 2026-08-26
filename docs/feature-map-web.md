# Feature map — web

**Start here to find code; this file answers "where does X live", not "why".**

One row per user-visible feature of the web app, giving the files it spans across
the four Rust crates plus its E2E spec. For *why* a module is shaped the way it
is, read [architecture.md](architecture.md). For the rules a change must obey,
read [.claude/rules/](../.claude/rules/).

The Android shell (`mobile/`) renders this same `frontend/` markup in a WebView
with `features = ["mobile"]`, so its features are the ones below. The native iOS
client is a separate codebase — see [feature-map-ios.md](feature-map-ios.md).

## Reading a row

Paths are **relative to each column's crate root**, so the `db/` cell
`annotations/` means `db/src/annotations/`:

| Column | Root | What's in it |
|---|---|---|
| `shared/` | `shared/src/` | serde wire types both sides agree on |
| `db/` | `db/src/` | SQL, queries, the indexing pipeline |
| `server/` | `server/src/` | REST handlers (mobile) + the route it answers |
| `frontend/` | `frontend/src/` | UI pages, `data/` transport, `rpc/` server fns |
| E2E | `ui_tests/playwright/tests/flows/` | the spec that covers it |

A blank cell means that layer genuinely isn't involved. Web-only features skip
`server/` entirely — they're Dioxus server functions under `frontend/src/rpc/`,
reachable at `/api/rpc/*`, and mobile never calls them.

**The two transports are not interchangeable.** `frontend/src/data/` is the
platform-agnostic wrapper every page calls; under `web`/`server` it proxies the
`rpc/` server function, under `mobile` it hits the `/api/*` route with `reqwest`.
Adding a read usually means touching both sides of that split — see the
[add-backend-route](../.claude/skills/add-backend-route/SKILL.md) skill.

## Library & browse

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| Landing (marquee, shelves row, cover wall) | `view_prefs/`, `discovery.rs` | `books/page.rs`, `books/facets.rs` | `backend/ebooks/` · `/api/ebooks`, `/api/library` | `pages/landing/`, `data/books/library.rs`, `rpc/books.rs` | `landing.spec.ts` |
| Sort / filter / persisted view prefs | `view_prefs/` | `books/page.rs`, `sort_keys.rs` | | `view_prefs.rs`, `pages/landing/sorting/`, `pages/landing/filtering/`, `client_store.rs` | `persistence.spec.ts` |
| Full-text search + ⌘K palette | `discovery.rs` | `books/search.rs` | `backend/search/` · `/api/search`, `/api/search/palette` | `components/search_palette/`, `pages/search.rs`, `rpc/palette.rs` | `search.spec.ts`, `search-palette.spec.ts` |
| Authors & series (index + detail) | `discovery.rs` | `browse/`, `discovery/` | `backend/authors/`, `backend/series/` · `/api/authors*`, `/api/series*` | `pages/authors_index/`, `pages/author.rs`, `pages/series_index/`, `pages/series.rs`, `data/authors.rs`, `data/series.rs`, `index_prefs.rs` | `browse_indices.spec.ts`, `discovery.spec.ts` |
| Tags & genres | `discovery.rs` | `taxonomy.rs` | `backend/tags/`, `backend/genres/` · `/api/tags`, `/api/genres` | `pages/tag_cloud.rs`, `data/tags.rs`, `data/genres.rs`, `components/chip_editor.rs` | `genres.spec.ts` |
| Shelves (smart / manual / wishlist) | `shelves/` | `shelves/` | `backend/shelves/` · `/api/shelves*` | `pages/shelf_detail/`, `pages/shelves_index.rs`, `pages/landing/shelf_gallery/`, `components/shelves_rail/`, `components/shelf_rule_builder.rs`, `data/shelves.rs`, `rpc/shelves/` | `shelves.spec.ts`, `shelf_gallery.spec.ts` |
| Book detail page | `ebook/` | `books/get.rs` | `backend/ebooks/` · `/api/ebooks/{uuid}` | `pages/book_detail/` | `book_detail.spec.ts`, `book_detail_chips.spec.ts` |
| Covers & thumbnails | `image_format.rs` | `covers/`, `thumbs/`, `palette/` | `backend/covers/` · `/api/covers/{uuid}`, `/api/thumbs/{uuid}/{size}` | `components/cover_tile.rs`, `pages/metadata_edit/cover_editor.rs`, `contexts/` | `thumbnails.spec.ts` |
| Author photos | `discovery.rs` | `author_photos/`, `author_photos_data/` | `backend/author_photos/` | `components/author_photo_edit.rs`, `data/authors.rs` | `author_photo.spec.ts` |

## Editing the library

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| Metadata editing / overrides | `ebook/overrides.rs` | `metadata_overrides/` | `backend/overrides/` | `pages/metadata_edit/`, `rpc/overrides.rs`, `data/books/manage.rs` | `metadata_edit.spec.ts` |
| Edition picker (provider fan-out) | `metadata_lookup/` | `metadata_lookup/` | `backend/metadata/` · `/api/metadata/providers` | `pages/metadata_edit/metadata_search/`, `data/metadata_search.rs`, `rpc/metadata_search.rs` | `metadata_edit_search.spec.ts` |
| Bulk + inline edit (table view) | `ebook/overrides.rs` | `metadata_overrides/` | `backend/overrides/` | `pages/landing/bulk_edit/`, `pages/landing/table/` | `landing_bulk_edit.spec.ts`, `landing_inline_edit.spec.ts` |
| Merge books / attach formats | `merge.rs`, `cross_format/` | `merge/`, `cross_format/` | `backend/cross_format/` | `components/merge_dialog.rs`, `components/format_switcher/`, `pages/book_detail/merge.rs`, `data/cross_format.rs` | `merge.spec.ts`, `cross_format_link.spec.ts` |
| Delete book / author | `deletion.rs` | `deletion/`, `missing_files/` | `backend/ebooks/`, `backend/authors/` | `components/delete_book_dialog/`, `pages/book_detail/delete.rs` | `book_delete.spec.ts`, `author_delete.spec.ts` |
| Add your own books (upload) | `upload.rs` | `identity/` | `backend/uploads/` · `/api/uploads/ebooks` | `pages/add_books.rs`, `components/add_books_sheet.rs`, `data/uploads.rs` | `add_books.spec.ts`, `add_books_sheet.spec.ts` |
| Library cleanup (dedup review) | `cleanup/` | `cleanup/` | *rpc-only* · `/api/rpc/cleanup/*` | `pages/cleanup_review/`, `pages/settings/cleanup.rs`, `data/cleanup.rs`, `rpc/cleanup/` | `cleanup_review.spec.ts` |

## Reading & listening

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| EPUB reader | `ebook/`, `progress/` | `epub_structure/`, `epub_rewrite/` | `backend/ebooks/` · `/api/ebooks/{uuid}/file` | `pages/reader/`, `reader_progress.rs` | `reader.spec.ts` |
| Comic (CBZ) reader | `ebook/` | `comic/` | `backend/ebooks/` | `pages/comic_reader.rs` | `comic_reader.spec.ts` |
| Audiobook player + mini dock | `audiobook.rs` | `audiobook/`, `hls/` | `backend/audiobooks/` | `pages/listen/`, `audiobook_progress.rs` | `listen.spec.ts`, `mini-dock.spec.ts` |
| Progress sync | `progress/` | `progress/` | `backend/progress/` · `/api/progress*` | `data/progress.rs`, `rpc/progress.rs`, `reader_progress.rs` | `persistence.spec.ts` |
| Immersive read / cross-format follow | `cross_format/` | `cross_format/` | `backend/cross_format/` | `pages/book_detail/immersive.rs`, `pages/book_detail/sync_link/`, `components/alignment_modal/` | `immersive.spec.ts`, `cross_format_prompts.spec.ts` |
| Highlights & notes | `highlight/` | `annotations/` | `backend/highlights/` · `/api/highlights*` | `pages/reader/highlights_drawer/`, `pages/book_detail/highlights/`, `data/highlights.rs`, `rpc/highlights.rs` | `reader.spec.ts` |
| Bookmarks | `bookmark/` | `bookmarks/` | `backend/bookmarks/` · `/api/bookmarks*` | `pages/reader/reader_bookmarks.rs`, `pages/listen/bookmarks.rs`, `data/bookmarks.rs` | `reader.spec.ts`, `listen.spec.ts` |
| Quote cards | `highlight/` | | | `components/quote_card/`, `pages/reader/quote_panel.rs` | `book_detail.spec.ts` |
| Read status (want / reading / finished) | `read_status/` | `read_status/` | `backend/read_status/` · `/api/read-status*` | `read_status_auto/`, `pages/book_detail/read_status.rs`, `data/read_status.rs` | `book_detail.spec.ts` |
| Reading sessions | `progress/` | `progress/session.rs` | `backend/progress/` · `/api/progress/sessions` | `session_tracker/` | `stats.spec.ts` |
| Sleep timer | | | | `pages/listen/sleep.rs`, `platform_sleep.rs` | `listen.spec.ts` |
| Reader typography & prefs | | | | `pages/reader/prefs/`, `pages/reader/typography.rs`, `pages/reader/aa_panel.rs` | `reader.spec.ts` |

## Personal shelf data

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| Star ratings | `ratings/` | `ratings/` | `backend/ratings/` · `/api/ratings*` | `pages/book_detail/rating.rs`, `data/ratings.rs` | `book_detail.spec.ts` |
| Reading journals | `journal/` | `journals/` | `backend/journals/` · `/api/journals*` | `pages/book_detail/journal/`, `data/journals.rs` | `journal.spec.ts` |
| Reading stats & insights | `stats/` | `stats/` | `backend/stats/` · `/api/stats` | `pages/stats/`, `data/stats.rs`, `data/insights.rs` | `stats.spec.ts` |
| Physical check-in & wishlist | `physical/`, `scan/`, `isbn/` | `physical/`, `scan/` | `backend/physical/`, `backend/scan/` · `/api/physical/*`, `/api/scan/*` | `pages/check_in/`, `pages/book_detail/physical/`, `components/barcode_scanner/`, `data/physical.rs`, `data/scan.rs` | `check_in_*.spec.ts`, `physical_collection.spec.ts` |
| "Readers also enjoyed" | `suggestion/` | `suggestions/` | `backend/suggestions/` | `pages/book_detail/discovery.rs`, `data/suggestions.rs` | `suggestions.spec.ts` |
| Fetch summary | `summary.rs` | `book_summary/` | `backend/summary/` · `/api/summary/sources` | `components/fetch_summary.rs`, `data/summary.rs` | `fetch_summary.spec.ts` |

## Getting books off the server

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| Send to Kindle (SMTP) | `settings/` | `kindle/` | `backend/kindle/` · `/api/kindle/*`, `/api/smtp*` | `data/kindle.rs`, `pages/settings/smtp.rs` | `settings.spec.ts` |
| Send to Kobo (KEPUB) | `kobo.rs` | `kepub/`, `kobo_devices/` | `backend/ebooks/` · `/api/ebooks/{uuid}/kepub` | `pages/account/kobo.rs`, `data/kobo.rs`, `rpc/kobo.rs` | `account.spec.ts` |
| Kobo sync API (device-facing) | `kobo.rs` | `kobo/`, `kobo_position/` | `backend/kobo/` · `/kobo/{token}/v1/*` | | *none* — [docs/kobo-smoke-test.md](kobo-smoke-test.md) |
| OPDS catalog | `opds/` | `books/`, `browse/` | `backend/opds/` · `/opds*`, `/opds/v2*` | | *none* |
| Format conversion (Calibre) | `settings/` | `convert/` | worker task `ConvertFormat` | `pages/settings/ebook_convert/` | `book_detail.spec.ts` |
| Hidden formats | `auth.rs` | `auth/` | `backend/account/` · `/api/account/hidden-formats` | `pages/account/hidden_formats.rs` | `hidden_formats.spec.ts` |
| Byte serving & validators | `ebook/validator.rs`, `http_range.rs` | `books/` | `backend/conditional/` | | — rule [09](../.claude/rules/09-content-validators.md) |

## Accounts & administration

| Feature | `shared/` | `db/` | `server/` | `frontend/` | E2E |
|---|---|---|---|---|---|
| Login / register / logout | `auth.rs` | `auth/` | `auth/` (not `backend/`) · `/api/auth/*` | `pages/auth/`, `components/auth/`, `data/auth.rs` | `auth.spec.ts` |
| Users & permissions (admin) | `auth.rs` | `auth/` | `backend/users/` · `/api/users*` | `pages/settings/users/`, `data/users.rs` | `users.spec.ts` |
| Sessions & devices | `auth.rs` | `auth/` | `backend/admin_sessions/` · `/api/auth/sessions*` | `pages/account/sessions.rs`, `data/sessions.rs`, `data/admin_sessions.rs` | `account.spec.ts` |
| Profile & avatar | `auth.rs` | `auth/` | `backend/profile/` · `/api/account/profile`, `/api/account/avatar` | `pages/account/profile/`, `components/user_avatar/`, `data/profile.rs` | `account.spec.ts`, `user_menu.spec.ts` |
| Settings page | `settings/` | `settings/` | `backend/settings/` · `/api/settings` | `pages/settings/`, `data/books/admin.rs` | `settings.spec.ts` |
| Indexer / scanner / worker | `worker/` | `scanner/`, `indexer/`, `sync/`, `worker/` | `backend/settings/` · `/api/reindex`, `/api/scan-library` | `components/worker_status/`, `pages/settings/background_tasks.rs`, `data/background_tasks.rs` | `worker-status.spec.ts` |
| Log viewer | `logs/` | `logs/` | *rpc-only* | `pages/logs.rs`, `data/logs.rs`, `rpc/logs.rs` | `logs.spec.ts` |
| Admin server health | `admin_health.rs` | `admin_health/` | `backend/admin_health/` · `/api/admin/health` | `pages/admin_health/`, `data/admin_health.rs` | `admin_health.spec.ts` |
| Theme / Atrium design system | | | | `components/atrium.rs`, `frontend/assets/atrium.css` | `theme_toggle.spec.ts` |
| Rate limiting, CSRF, headers | | | `rate_limit/`, `auth/csrf/`, `security_headers/` | | `auth.spec.ts` |

## Coverage gaps worth knowing

- **OPDS has no E2E spec at all** — `server/src/backend/opds/tests/` is the only coverage.
- **The Kobo sync API has no automated test** — it's a manual runbook, [docs/kobo-smoke-test.md](kobo-smoke-test.md).
- **`/shelves` and `/shelves/:id` are mobile-only routes.** Shelf delete, add-books, and the member sort control have no web E2E coverage by design — see [rule 04](../.claude/rules/04-playwright.md).
- **`frontend/src/offline/` is mobile-gated** (`#[cfg(feature = "mobile")]`) and never compiles for web. It backs the Android shell; see [rule 08](../.claude/rules/08-offline-writes.md).
