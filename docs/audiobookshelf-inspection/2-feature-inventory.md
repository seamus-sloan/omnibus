# 2. AudioBookShelf feature inventory

## Tech stack

From [package.json](https://github.com/advplyr/audiobookshelf/blob/master/package.json): Node.js 20, **Express 4.17** as the HTTP layer, **Sequelize 6.35** ORM on top of **sqlite3 5.1**, **socket.io 4.5** for real-time events, **Passport 0.6** + `passport-jwt` + `openid-client` for auth, `fluent-ffmpeg` (vendored under `server/libs/fluentFfmpeg`) shelling out to ffmpeg/ffprobe binaries, `node-cron` for scheduling, `nodemailer` for SMTP, `xml2js` + `rss` for feed I/O, `umzug` + custom `MigrationManager` for versioned migrations. The Nuxt 2 + Vue 2 client ([client/package.json](https://github.com/advplyr/audiobookshelf/blob/master/client/package.json)) ships `hls.js`, `epubjs`, `@teckel/vue-pdf`, `libarchive.js` (CBZ/CBR), `nuxt-socket-io`, `vuedraggable`. Tests: mocha + chai + sinon + nyc; client tests use Cypress component mode.

## Libraries (multi, per media type)

[server/models/Library.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Library.js) is keyed on `mediaType` = `book` or `podcast`. Each library owns many `LibraryFolder` rows. Per-library `settings` JSON carries `coverAspectRatio`, `autoScanCronExpression`, `skipMatchingMediaWithAsin/Isbn`, `audiobooksOnly`, `hideSingleBookSeries`, `onlyShowLaterBooksInContinueSeries`, `metadataPrecedence` (ordered list: `folderStructure`, `audioMetatags`, `nfoFile`, `txtFiles`, `opfFile`, `absMetadata`), and `markAsFinishedPercentComplete` / `markAsFinishedTimeRemaining`. Cron is per-library, not global — so each library runs on its own schedule.

## Browse / filter / sort / search

Primary endpoint is `GET /api/libraries/:id/items` in [LibraryController.getLibraryItems](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/LibraryController.js). Query params: `limit`, `page`, `sort`, `desc=1`, `filter` (dotted `group.value`, e.g. `authors.<id>`, `genres.<genre>`, `progress.finished`, `series.<id>`, `issues`, `feed-open`), `collapseseries`, `include=rssfeed,progress,media,authors,series,numEpisodes`. The actual work is composed in [server/utils/queries/libraryItemsBookFilters.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/queries/libraryItemsBookFilters.js) (~1000 lines of Sequelize literal-SQL), which issues separate find queries for filter groups and then joins with `include`. There is `GET /api/libraries/:id/search` (fuse.js + LIKE; see [SearchController](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/SearchController.js)) and an optional accent/case-folding path if the user ships the `nusqlite3` extension (`Database.supportsUnaccent`, [Database.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js)). **There is no FTS5 virtual table.**

Personalized home shelves (`continue-listening`, `continue-series`, `newest-authors`, `recently-added`, `listen-again`, etc.) are assembled in [server/utils/queries/libraryFilters.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/queries/libraryFilters.js) + [seriesFilters.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/queries/seriesFilters.js) via `GET /api/libraries/:id/personalized`.

## LibraryItem detail, metadata edit

`GET /api/items/:id` → [LibraryItemController.findOne](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/LibraryItemController.js). `PATCH /api/items/:id/media` edits the embedded Book/Podcast (title, subtitle, description, authors, narrators, series, tags, genres, publishedYear, isbn/asin, language, chapters). `POST /api/items/:id/match` fans out to the configured provider and overwrites fields based on per-field precedence. Cover: `POST/PATCH/DELETE /api/items/:id/cover` (upload or URL), handled by [CoverManager](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/CoverManager.js).

## Upload / scan / matching

Scanner lives under [server/scanner/](https://github.com/advplyr/audiobookshelf/tree/master/server/scanner): `LibraryScanner` (orchestrator, ~700 lines), `LibraryItemScanner` (single item), `BookScanner` (999 lines — the metadata-precedence engine), `PodcastScanner`, `AudioFileScanner` (ffprobe wrapper with chapter extraction), `OpfFileScanner`, `NfoFileScanner`, `AbsMetadataFileScanner` (reads/writes `.abs` JSON sidecar). `POST /api/libraries/:id/scan` triggers via [LibraryController.scan](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/LibraryController.js). "Quick match" at `POST /api/items/:id/match` and `POST /api/libraries/:id/matchall` call [BookFinder](https://github.com/advplyr/audiobookshelf/blob/master/server/finders/BookFinder.js) or [PodcastFinder](https://github.com/advplyr/audiobookshelf/blob/master/server/finders/PodcastFinder.js). Live filesystem changes are picked up by [Watcher.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Watcher.js) (the `watcher` npm package, 10s debounce).

## Audio metadata / chapters

[server/scanner/AudioFileScanner.js](https://github.com/advplyr/audiobookshelf/blob/master/server/scanner/AudioFileScanner.js) invokes `ffprobe` via [server/utils/prober.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/prober.js) to read tags (title/artist/album/year/track/disc/narrator/composer/description/comment) and embedded chapters. Fallbacks: overdrive MediaMarkers in [parseOverdriveMediaMarkers.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/parsers/parseOverdriveMediaMarkers.js), one-chapter-per-file for multi-file books. Writing tags back into m4b/mp3 is [AudioMetadataManager](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/AudioMetadataManager.js), exposed through `POST /api/tools/item/:id/embed-metadata`.

## Ebook support

Per [server/utils/globals.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/globals.js): `SupportedEbookTypes = ['epub','pdf','mobi','azw3','cbr','cbz']`. Ebook reading is client-side (`epubjs`, `@teckel/vue-pdf`, `libarchive.js`). `GET /api/items/:id/ebook/:fileid?` streams the raw file ([LibraryItemController.getEBookFile](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/LibraryItemController.js)); per-user ebook progress (`ebookLocation`, `ebookProgress`) lives on `MediaProgress`. "Supplementary" ebooks (a PDF accompanying an m4b) are flagged `isSupplementary` on the `LibraryFile`.

## Podcasts — RSS ingestion, auto-download

[server/utils/podcastUtils.js](https://github.com/advplyr/audiobookshelf/blob/master/server/utils/podcastUtils.js) has `getPodcastFeed(feedUrl)` (axios GET, iso-8859-1 fallback, `xml2js` → normalized JSON) and `parsePodcastRssFeedXml`. [PodcastManager](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/PodcastManager.js) holds a `downloadQueue` with a **single `currentDownload`** — all podcast episodes download one at a time. `autoDownloadEpisodes` + `autoDownloadSchedule` (cron) on `Podcast` drive scheduled polling via [CronManager.initPodcastCrons](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/CronManager.js). `maxEpisodesToKeep` + `maxNewEpisodesToDownload` cap volume. OPML import/export: `POST /api/podcasts/opml/parse`, `/opml/create`, `GET /api/libraries/:id/opml`.

## Streaming / HLS / direct play / transcoding

[server/objects/Stream.js](https://github.com/advplyr/audiobookshelf/blob/master/server/objects/Stream.js) builds an ffmpeg concat input from the book's audio files and emits HLS (`.m3u8` + `.ts` or fMP4). Segment length 6s; AAC is forced for FLAC and certain ac3/eac3 sources. `GET /hls/:stream/:file` ([HlsRouter](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/HlsRouter.js)) serves segments from `<MetadataPath>/streams/<streamId>/`. Clients that support the native codec can "direct play" by hitting `GET /public/session/:id/track/:index` ([PublicRouter](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/PublicRouter.js)) — no transcode. `X-Accel-Redirect` offload is supported via `USE_X_ACCEL` env var.

## Progress sync, bookmarks, playback sessions

`MediaProgress` (user × book-or-episode) tracks `currentTime`, `duration`, `isFinished`, `hideFromContinueListening`, `finishedAt`, `ebookLocation`, `ebookProgress`. `PATCH /api/me/progress/:libraryItemId/:episodeId?` is the hot path; `PATCH /api/me/progress/batch/update` supports offline sync. Bookmarks are per-user JSON blobs on the user row (`bookmarks` column on [User.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/User.js)) keyed by library item id + time. `PlaybackSession` records one row per listening session for stats (displayTitle, timeListening, date, dayOfWeek, mediaPlayer). Mobile apps do their own local sessions and POST them up via `POST /api/session/local` and `/session/local-all` in [SessionController](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/SessionController.js).

## OPDS / RSS feed output

No OPDS. The `Feed` model + [RssFeedManager](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/RssFeedManager.js) publishes **podcast-style RSS** for arbitrary audiobooks/collections/series/playlists so they can be consumed by regular podcast apps. Endpoints: `POST /api/feeds/item/:itemId/open`, `/collection/:collectionId/open`, `/series/:seriesId/open`, `POST /api/feeds/:id/close`. Public delivery at `GET /feed/:slug`, `/feed/:slug/cover*`, `/feed/:slug/item/:episodeId/*` (registered in [Server.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Server.js)).

## Auth — local + OIDC

[server/Auth.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Auth.js) mounts passport-jwt as the middleware at `/api`. [LocalAuthStrategy.js](https://github.com/advplyr/audiobookshelf/blob/master/server/auth/LocalAuthStrategy.js) uses bcryptjs. [OidcAuthStrategy.js](https://github.com/advplyr/audiobookshelf/blob/master/server/auth/OidcAuthStrategy.js) uses `openid-client` with full PKCE + session-state map. [TokenManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/auth/TokenManager.js) mints access + refresh JWTs; the secret is persisted in the Setting table. API keys are a separate `ApiKey` model + [ApiKeyController](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/ApiKeyController.js). User roles are strings (`root`, `admin`, `user`, `guest`) with a `permissions` JSON blob (download, update, delete, upload, accessExplicitContent, accessAllLibraries, accessAllTags, selectedTagsNotAccessible, allowedLibraries[], itemTagsSelected[]).

## Collections, Playlists, Series, Authors, Tags, Genres

Proper Sequelize models + m2m link tables: `Collection`+`CollectionBook`, `Playlist`+`PlaylistMediaItem` (user-scoped), `Series`+`BookSeries` (with `sequence` column), `Author`+`BookAuthor`. Tags and genres are **JSON arrays** on `Book`/`Podcast` (not tables) — renaming a tag requires scanning all rows (`POST /api/tags/rename` in [MiscController](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/MiscController.js)).

## Metadata providers

[server/providers/](https://github.com/advplyr/audiobookshelf/tree/master/server/providers): `Audible.js`, `Audnexus.js` (authors+series), `iTunes.js`, `GoogleBooks.js`, `OpenLibrary.js`, `MusicBrainz.js`, `FantLab.js` (Russian), `AudiobookCovers.js`, `CustomProviderAdapter.js` (user-registered HTTP endpoint, schema in [custom-metadata-provider-specification.yaml](https://github.com/advplyr/audiobookshelf/blob/master/custom-metadata-provider-specification.yaml)). Aggregated by [BookFinder.js](https://github.com/advplyr/audiobookshelf/blob/master/server/finders/BookFinder.js) (702 lines) with levenshtein-distance ranking.

## Stats, notifications, backups, sharing

Per-user stats at `GET /api/me/listening-stats` and `/me/stats/year/:year` ([MeController](https://github.com/advplyr/audiobookshelf/blob/master/server/controllers/MeController.js)); admin aggregates at `/api/stats/server`. Notifications use **Apprise** via HTTP POST to `Database.notificationSettings.appriseApiUrl` ([NotificationManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/NotificationManager.js)) — events: `onPodcastEpisodeDownloaded`, `onBackupCompleted`, `onBackupFailed`, `onRSSFeedFailed`, `onRSSFeedDisabled`, `onTest`. Backups tar the SQLite file + metadata ([BackupManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/BackupManager.js)). Public share links for individual media items via `MediaItemShare` + [PublicRouter](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/PublicRouter.js).

## Web player

Nuxt 2 SPA under [client/](https://github.com/advplyr/audiobookshelf/tree/master/client). HLS playback via `hls.js`, native `<audio>` for direct play. Cypress component tests. A `REACT_CLIENT_PATH` env var swaps in an experimental Next.js client (see [Server.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Server.js)).

---

[← Omnibus state](1-omnibus-state.md) · [Next: performance pain points →](3-performance-pain-points.md)
