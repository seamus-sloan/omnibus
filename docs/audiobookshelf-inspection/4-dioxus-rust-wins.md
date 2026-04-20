# 4. Where Dioxus / Rust wins

Given Omnibus' stack (axum + sqlx + Dioxus fullstack):

- **True parallelism on scans.** ABS's library scan is bottlenecked on serial `ffprobe` invocations in a single event loop. Omnibus can run N ffprobes concurrently via `tokio::process::Command` + `JoinSet` bounded by a `Semaphore(num_cpus)`. A 5000-item library drops from "go make coffee" to minutes. The Watcher pattern in [Watcher.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Watcher.js) maps cleanly onto the `notify` crate with a debounced channel.

- **FFmpeg orchestration parallelism.** ABS spawns one ffmpeg per HLS session with no cache. Omnibus can share HLS segments across sessions keyed on `(book_id, codec_profile, segment_index)`, store them under `<data_dir>/hls-cache/`, and let two users on the same book pull from the same segment files. The `image` crate + `symphonia` can even probe without shelling out for common formats, reserving ffmpeg for actual transcode.

- **sqlx vs Sequelize.** The 1000-line `libraryItemsBookFilters.js` composes raw SQL through `Sequelize.literal` anyway — Omnibus can write the same SQL with compile-time verification and `#[derive(FromRow)]` structs, skipping ORM object graph hydration. No `toOldJSON()` / `toOldJSONExpanded()` conversion layers like ABS carries as a v1-compat tax ([SocketAuthority.js](https://github.com/advplyr/audiobookshelf/blob/master/server/SocketAuthority.js), [PlaybackSession.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/PlaybackSession.js)).

- **FTS5 unconditionally.** ABS makes operators install `nusqlite3` out-of-band for accent-folding and never ships FTS. Omnibus creates `CREATE VIRTUAL TABLE library_items_fts USING fts5(title, subtitle, authors, narrators, series, description, tokenize='unicode61 remove_diacritics 2', content='library_items', content_rowid='rowid')` with AFTER INSERT/UPDATE/DELETE triggers at `initialize_schema` time. bm25 ranking comes for free.

- **SSR + WASM hydration for the web player.** Nuxt 2 is end-of-life (Vue 2 sunset reached Dec 2023). Dioxus fullstack renders the library grid server-side and hydrates; Dioxus signals can re-filter a pre-fetched `Signal<Vec<LibraryItem>>` without network round-trips. The `hls.js` dependency stays — there's no WASM replacement yet — but everything else becomes typed Rust.

- **Proper socket scoping.** ABS iterates every client per event. axum + `tokio::sync::broadcast` scoped per library id means a bulk scan broadcasts once to subscribers, not N² to everyone. And with typed `ServerEvent` enums there's no "toOldJSONExpanded" duplication.

- **No Sequelize auto-sync at boot.** `Database.sequelize.sync({ force, alter: false })` at boot + `ANALYZE` costs seconds. sqlx with `sqlx-migrate` (or `refinery`) runs versioned migrations only on change; the rest of boot is connect-and-ping.

- **Memory & startup.** ABS idles at 150–200 MB RSS; an equivalent Rust binary idles at 20–40 MB. On a Synology DS220+ that's the difference between "ABS + Jellyfin + Immich" and "ABS *or* Jellyfin."

- **Typed schema across the wire.** Omnibus can share `#[derive(Serialize)]` structs between `server/`, `frontend/`, and `mobile/` (already the pattern in [omnibus-shared](../../shared/src/lib.rs)), so a `MediaProgress` mismatch is a build break, not an integration-test discovery. ABS's mobile protocol is kept stable through `toOldJSON*` methods precisely because there's no shared contract.

- **Streaming OPML / feed responses.** ABS generates OPML and RSS as whole-string buffers. axum's `StreamBody` + `quick-xml` writer emits chunks progressively; a 500-podcast OPML export becomes constant-memory.

---

[← Performance pain points](3-performance-pain-points.md) · [Next: schema details →](5-schema-details.md)
