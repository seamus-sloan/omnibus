# 3. Performance pain points in AudioBookShelf

## Single-threaded everything

Node.js is one event loop. [PodcastManager.startPodcastEpisodeDownload](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/PodcastManager.js) serializes through `this.currentDownload` — a feed with 200 new episodes downloads them one at a time, and new items in the queue wait behind the active one. Library scans in [LibraryScanner.scanLibrary](https://github.com/advplyr/audiobookshelf/blob/master/server/scanner/LibraryScanner.js) do per-item ffprobe invocations serially; ffprobe on a long audiobook with many chapter atoms can take multiple seconds, and a fresh scan of a 5000-audiobook library can run for hours on a Synology. The Watcher's 10-second debounce ([Watcher.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Watcher.js)) masks it but doesn't fix it.

## HLS transcode is per-session

[Stream.js](https://github.com/advplyr/audiobookshelf/blob/master/server/objects/Stream.js) spawns one ffmpeg per open session. Two users listening to two FLAC books = two simultaneous `libfdk_aac` encodes. There's no segment cache — if the same user re-opens the book on a different device they re-transcode. Segments live under `<MetadataPath>/streams/<streamId>/` and are only cleaned up by the `closeStaleOpenSessions` cron ([CronManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/CronManager.js)) at `30 0 * * *`.

## Sequelize N+1 hotspots + heavy literal SQL

Per [server/utils/queries/libraryItemsBookFilters.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/queries/libraryItemsBookFilters.js) the authors have explicitly left `// TODO: Reduce queries` for the continue-series shelf. The same file issues raw correlated subqueries per row (`Sequelize.literal('(SELECT max(mp.updatedAt) FROM bookSeries bs, mediaProgresses mp WHERE mp.mediaItemId = bs.bookId AND mp.userId = :userId AND bs.seriesId = series.id)')`). Filter data is computed on every request unless cached in `Database.libraryFilterData` (an in-memory map with ad-hoc mutation — `replaceTagInFilterData`, `addTagsToFilterData`, etc. in [Database.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js)). Cache invalidation is manual per-mutation — see the `// TODO: Keep cached filter data up-to-date on updates` in [libraryFilters.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/queries/libraryFilters.js).

## Tags/genres are JSON arrays

`Book.tags` and `Book.genres` are `DataTypes.JSON` ([Book.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Book.js)). Renaming a tag requires scanning every Book row and rewriting JSON — see [MiscController.renameTag](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/MiscController.js). `GET /api/tags` aggregates in memory across all books.

## Socket.io broadcast fan-out

[SocketAuthority.emitter](https://github.com/advplyr/audiobookshelf/blob/master/server/SocketAuthority.js) iterates every connected client on every event. `libraryItemEmitter` calls `toOldJSONExpanded()` per client — a full library item re-serialization per recipient. With 20 clients watching a 500-item bulk scan, that's 10,000 serializations. No rooms, no per-library filtering at the socket layer.

## No FTS, no unaccent by default

Search uses `LIKE '%term%'` unless the operator installs the `nusqlite3` extension out-of-band (see `process.env.NUSQLITE3_PATH` in [Database.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js)). No FTS5 tables, no triggers, no bm25 ranking. Client-side `fuse.js` picks up the slack but at the cost of shipping the whole library to the browser.

## Runtime migration on startup

[Database.buildModels](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js) calls `this.sequelize.sync({ force, alter: false })` every boot. `alter: false` is deliberate; real migrations run via [MigrationManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/MigrationManager.js) using `umzug`. `ANALYZE` is run on every boot — fine for query planning but adds seconds on a large DB.

## Memory footprint

Running with Sequelize's ORM-object hydration + `xml2js` + `libarchive.js` in-worker + Nuxt SSR at dev time, the server idles around 150–200 MB RSS on small libraries and grows linearly with the in-memory `libraryFilterData` cache and `playbackSessionManager.sessions` array ([PlaybackSessionManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/PlaybackSessionManager.js)).

---

[← Feature inventory](2-feature-inventory.md) · [Next: Dioxus / Rust wins →](4-dioxus-rust-wins.md)
