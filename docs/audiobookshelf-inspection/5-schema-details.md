# 5. Schema details worth copying (and improving)

ABS uses a **single SQLite database** (`absdatabase.sqlite` in ConfigPath, see [Database.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Database.js)) with **Sequelize-managed tables**, every PK a UUIDv4, every mutable sub-structure a JSON column. Migrations via `umzug` + the files in [server/migrations/](https://github.com/advplyr/audiobookshelf/tree/master/server/migrations).

## Core media tables

From [server/models/Book.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Book.js):

```js
// book
id UUID PK
title STRING, titleIgnorePrefix STRING, subtitle STRING
publishedYear STRING, publishedDate STRING, publisher STRING
description TEXT, isbn STRING, asin STRING, language STRING
explicit BOOLEAN, abridged BOOLEAN
coverPath STRING, duration FLOAT
narrators JSON, audioFiles JSON, ebookFile JSON, chapters JSON
tags JSON, genres JSON
// indexes: title NOCASE, publishedYear, duration
```

From [LibraryItem.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/LibraryItem.js):

```js
// libraryItem — the filesystem-backed envelope around a book/podcast
id UUID PK
ino STRING (filesystem inode — scan continuity)
path STRING, relPath STRING
mediaId UUID, mediaType STRING ('book' | 'podcast')
isFile BOOLEAN, isMissing BOOLEAN, isInvalid BOOLEAN
mtime/ctime/birthtime DATE(6)
size BIGINT
lastScan DATE, lastScanVersion STRING
libraryFiles JSON, extraData JSON
title STRING (denormalized for sorting)
titleIgnorePrefix STRING
authorNamesFirstLast STRING, authorNamesLastFirst STRING
libraryId FK, libraryFolderId FK
```

The `libraryItem → book` relation uses `foreignKey: 'mediaId', constraints: false` ([LibraryItem.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/LibraryItem.js)) because `mediaId` polymorphically points at `book` or `podcast`. No DB-level FK enforcement on that edge — a known footgun. **Omnibus should split into two tables (`audiobook_items`, `podcast_items`) or use a discriminator with separate real FKs**, not polymorphic no-constraint links.

## Podcasts

```js
// podcast (from Podcast.js)
id UUID PK
title, titleIgnorePrefix, author, releaseDate
feedURL, imageURL, itunesPageURL, itunesId, itunesArtistId
description TEXT, language, podcastType, explicit
autoDownloadEpisodes BOOLEAN, autoDownloadSchedule STRING (cron)
lastEpisodeCheck DATE, maxEpisodesToKeep INTEGER, maxNewEpisodesToDownload INTEGER
coverPath, tags JSON, genres JSON, numEpisodes INTEGER

// podcastEpisode (from PodcastEpisode.js)
id UUID PK, podcastId FK
index INTEGER, season STRING, episode STRING, episodeType STRING
title, subtitle STRING(1000), description TEXT, pubDate STRING
enclosureURL, enclosureSize BIGINT, enclosureType
publishedAt DATE
audioFile JSON, chapters JSON, extraData JSON
```

## User + progress + sessions

```js
// user (User.js)
id UUID PK, username, email, pash, type ('root'|'admin'|'user'|'guest'), token
isActive BOOLEAN, isLocked BOOLEAN, lastSeen DATE
permissions JSON, bookmarks JSON, extraData JSON

// mediaProgress (MediaProgress.js)
id UUID PK, userId FK
mediaItemId UUID, mediaItemType STRING (polymorphic, no FK)
duration FLOAT, currentTime FLOAT
isFinished BOOLEAN, hideFromContinueListening BOOLEAN
ebookLocation STRING, ebookProgress FLOAT
finishedAt DATE, extraData JSON, podcastId UUID

// playbackSession (PlaybackSession.js) — one row per listening session, kept for stats
id UUID PK, userId FK, deviceId FK, libraryId FK
mediaItemId UUID, mediaItemType STRING (polymorphic)
displayTitle, displayAuthor STRING, duration FLOAT
playMethod INTEGER, mediaPlayer STRING
startTime/currentTime FLOAT
timeListening INTEGER, mediaMetadata JSON
date STRING, dayOfWeek STRING
```

Bookmarks live in `user.bookmarks` as a JSON array rather than their own table — which means a full row-rewrite on every bookmark add/remove and no FK to library items. **Omnibus should normalize** to a `bookmarks(user_id, library_item_id, time_seconds, title, created_at)` table with indices.

## Organization

- **Series** (UUID + name + nameIgnorePrefix + description, per-library), m2m link via **BookSeries** with a `sequence` TEXT column — [Series.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Series.js), [BookSeries.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/BookSeries.js).
- **Author** (name, lastFirst, asin, description, imagePath, per-library) m2m via **BookAuthor** ([Author.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Author.js)).
- **Collection** (library-scoped, admin-curated) + **CollectionBook** (ordered).
- **Playlist** (user-scoped, can mix books and podcast episodes) + **PlaylistMediaItem** with polymorphic `mediaItemId/mediaItemType`.
- **Tags** and **Genres**: *not tables*; JSON arrays on Book/Podcast. Renames scan every row.

## Infrastructure tables

- **ApiKey** — programmatic access tokens separate from user sessions.
- **Device** — `deviceId`, `clientName` ("Abs Web", "Abs Android"), `clientVersion`, `deviceName`, `ipAddress` ([Device.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Device.js)). One row per registered client.
- **Feed** + **FeedEpisode** — published RSS catalog for any entity (`entityType` = book/collection/series/playlist; [Feed.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Feed.js)).
- **MediaItemShare** — public share links.
- **Setting** — generic key/value for `ServerSettings`, `EmailSettings`, `NotificationSettings` serialized into JSON.
- **Session** — auth refresh-token store (rows per browser/device; see [v2.26.0-create-auth-tables.js](https://github.com/advplyr/audiobookshelf/blob/master/server/migrations/v2.26.0-create-auth-tables.js)).

## Indexes

Declared inline in each model's `indexes:` option — [Book indexes](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Book.js) on `title NOCASE`, `publishedYear`, `duration`. Retrofitted indexes in [v2.15.2-index-creation.js](https://github.com/advplyr/audiobookshelf/blob/master/server/migrations/v2.15.2-index-creation.js), [v2.17.7-add-indices.js](https://github.com/advplyr/audiobookshelf/blob/master/server/migrations/v2.17.7-add-indices.js), [v2.33.0-add-discover-query-indexes.js](https://github.com/advplyr/audiobookshelf/blob/master/server/migrations/v2.33.0-add-discover-query-indexes.js) — the retrofit trail is a map of which queries hurt in production.

## Migration approach

`umzug`-driven with filename convention `v<server-version>-<name>.js`, each exporting `up` and `down` as required by [migrations/readme.md](https://github.com/advplyr/audiobookshelf/blob/master/server/migrations/readme.md). Orchestrator in [MigrationManager.js](https://github.com/advplyr/audiobookshelf/blob/master/server/managers/MigrationManager.js). Run automatically on startup when `package.json.version` differs from the persisted `Setting.version` — down migrations when downgrading. This is a solid pattern Omnibus should mirror with `sqlx migrate` or `refinery` once the schema stabilizes.

---

[← Dioxus / Rust wins](4-dioxus-rust-wins.md) · [Next: API surface →](6-api-surface.md)
