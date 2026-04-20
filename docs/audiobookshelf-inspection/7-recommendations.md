# 7. Recommendations for Omnibus

Ordered roughly by payoff vs. cost. These compose with the Calibre-Web recommendations — where they conflict, the ABS recommendation wins for audiobook/podcast paths.

1. **Split `library_items` polymorphism into concrete tables.** ABS's `libraryItem.mediaId` points at either `book.id` or `podcast.id` with `constraints: false` ([LibraryItem.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/LibraryItem.js)) — no FK enforcement, same bug surface as Calibre's cross-DB orphan issue. Use two tables (`audiobook_items`, `podcast_items`) with real FKs, or a discriminator column plus concrete `book_id` / `podcast_id` nullable FKs with a CHECK constraint. Same argument for `mediaProgress.mediaItemId` and `playbackSession.mediaItemId`.

2. **Normalize tags and genres into tables.** ABS stores both as JSON arrays on Book/Podcast ([Book.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Book.js)) and pays for it at rename time ([MiscController.renameTag](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/MiscController.js)). A `tags(id, name)` + `item_tags(item_id, tag_id)` design makes `GET /api/tags` a single query, renames an `UPDATE tags SET name = ?`, and enables indexed filter-by-tag without JSON functions.

3. **FTS5 at startup with triggers.** ABS defers full-text to an optional `nusqlite3` extension ([Database.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js)) and falls back to LIKE/fuse.js. Omnibus creates `library_items_fts(title, subtitle, authors, narrators, series, description)` unconditionally with `tokenize='unicode61 remove_diacritics 2'` and AFTER INSERT/UPDATE/DELETE triggers. Ship bm25 as the primary search path; drop client-side fuse.

4. **Shared HLS segment cache keyed by `(book_id, codec_profile, segment_index)`.** ABS spawns one ffmpeg per session ([Stream.js](https://github.com/advplyr/audiobookshelf/blob/master/server/objects/Stream.js)) with no cache. Store segments under `<data_dir>/hls/<book_id>/<profile>/seg-NNN.ts`, serve from disk, populate on miss with a `tokio::sync::Mutex` per (book, profile) so only one ffmpeg runs per unique stream. Two users on the same book = one transcode.

5. **Scanner = `JoinSet` + `Semaphore(num_cpus)`.** Mirror BookScanner's metadata-precedence engine ([BookScanner.js](https://github.com/advplyr/audiobookshelf/blob/master/server/scanner/BookScanner.js)) but parallelize `ffprobe` invocations. On a 5000-item library, the walltime gap vs. ABS's serial scan is ~linear in `num_cpus`. Keep the precedence list itself (folderStructure → audioMetatags → nfoFile → opfFile → absMetadata → providerOverride) — it's well-thought-out.

6. **Persist bookmarks in their own table.** ABS's `user.bookmarks` JSON column rewrites the whole user row on every add/remove ([User.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/User.js)). Omnibus: `bookmarks(id, user_id, library_item_id, time_seconds, title, created_at)` with `(user_id, library_item_id)` index.

7. **Typed Socket protocol over broadcast channels.** Replace ABS's `toOldJSONExpanded()` per-client serialization loop ([SocketAuthority.js](https://github.com/advplyr/audiobookshelf/blob/master/server/SocketAuthority.js)) with `ServerEvent` enum + `tokio::sync::broadcast` scoped per library. Subscribers filter at receive time; payloads serialize once per event. Same shape across server/frontend/mobile from a shared crate (already the pattern in [omnibus-shared](../../shared/src/lib.rs)).

8. **Podcast downloads: concurrent with per-feed fairness.** ABS serializes through `currentDownload` ([PodcastManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/PodcastManager.js)). Omnibus: a bounded `JoinSet` with a per-feed guard so one slow feed can't starve others. Round-robin pull from `downloadQueue` grouped by `podcast_id`.

9. **Adopt the Metadata Precedence list as a user-facing setting.** ABS's per-library `metadataPrecedence` JSON array is one of its best UX decisions ([Library.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Library.js)). Copy verbatim: the user controls which source wins per field and per library, not a hard-coded priority.

10. **Copy ABS's `umzug`-style migration approach, not Calibre-Web's runtime ALTER.** Filename convention `v<version>-<name>.rs` with `up`/`down`, run on boot when the stored version differs from `cargo.toml`. `refinery` or `sqlx migrate` both do this cleanly. See ABS's [MigrationManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/MigrationManager.js) and the index-retrofit trail in [server/migrations/](https://github.com/advplyr/audiobookshelf/tree/master/server/migrations) — that list is a free hint sheet for indexes you'll need.

11. **OIDC from day one, not a bolt-on.** [OidcAuthStrategy.js](https://github.com/advplyr/audiobookshelf/blob/master/server/auth/OidcAuthStrategy.js) is ~560 lines; the patterns (PKCE, state map, group-based role mapping, mobile redirect subfolder handling) are worth copying structurally. Rust: `openidconnect` crate.

12. **Publish `/api/*` as the canonical REST contract and document it.** ABS's unified `/api/*` for both mobile and web ([ApiRouter.js](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/ApiRouter.js)) is better than Calibre-Web's "OPDS + Kobo + ad-hoc /ajax" split. Use `utoipa` or `aide` to emit OpenAPI from the axum handlers so the mobile team doesn't have to read source.

13. **Ship podcast-as-RSS publishing.** [RssFeedManager](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/RssFeedManager.js) lets any audiobook/collection/series look like a podcast to Overcast/AntennaPod. Low-cost to implement on top of `quick-xml`, high user value for commute-during-audiobook use cases. Mount under `/feed/:slug` outside the auth middleware.

14. **Apprise for notifications, not bespoke integrations.** ABS POSTs a simple `{ urls, title, body }` payload to an operator-run Apprise HTTP server ([NotificationManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/NotificationManager.js)). Omnibus inherits 80+ notification targets for one HTTP client call.

15. **Streaming responses for OPML, feed, bulk-download.** ABS generates OPML as whole-string buffers. axum `StreamBody` + `quick-xml` writer on `tokio::io::DuplexStream` — constant memory for a 10k-podcast export. Same technique for bulk ZIP download (`/api/libraries/:id/download`) using `async-zip`.

## Cross-reference to roadmap

| Roadmap initiative | Recommendations above |
|---|---|
| [F0.1 Schema refactor](../roadmap/0-1-schema-refactor.md) | 1, 2, 6 |
| [F0.2 Migrations](../roadmap/0-2-migrations.md) | 10 |
| [F0.3 Auth](../roadmap/0-3-auth.md) | 11 |
| [F0.4 FTS5](../roadmap/0-4-fts5.md) | 3 |
| [F0.5 Background worker](../roadmap/0-5-background-worker.md) | 5, 8 |
| [F0.6 Library filesystem](../roadmap/0-6-library-filesystem.md) | 5, 9 |
| [F1.1 Search](../roadmap/1-1-search.md) | 3 |
| [F1.3 Library views](../roadmap/1-3-library-views.md) | 7 |
| [F2.x Audio streaming / HLS](../roadmap/0-0-summary.md) | 4, 15 |
| [F2.x Podcasts](../roadmap/0-0-summary.md) | 8, 13 |
| [F3.x Progress sync](../roadmap/0-0-summary.md) | 6, 7 |
| [F4.x Feeds / sharing](../roadmap/0-0-summary.md) | 13, 15 |
| [F5.1 Metadata edit](../roadmap/5-1-metadata-edit.md) | 9 |
| Admin | 12, 14 |

---

[← API surface](6-api-surface.md) · [Overview](0-overview.md) · [Roadmap summary](../roadmap/0-0-summary.md)
