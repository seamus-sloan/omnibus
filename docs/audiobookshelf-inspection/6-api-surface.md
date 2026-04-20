# 6. API surface

## REST — single `/api/*` router

[server/routers/ApiRouter.js](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/ApiRouter.js) mounts ~150 routes, grouped by controller. Authentication is `passport.authenticate('jwt', { session: false })` for everything except `GET /api/items/:id/cover` and `GET /api/authors/:id/image` (unauth via [Auth.authNotNeeded](https://github.com/advplyr/audiobookshelf/blob/master/server/Auth.js)).

Notable route families:

- **Libraries**: `/libraries` CRUD, `/libraries/:id/items`, `/search`, `/personalized`, `/filterdata`, `/series`, `/collections`, `/playlists`, `/authors`, `/narrators`, `/scan`, `/matchall`, `/recent-episodes`, `/opml`, `/stats`, `/download`, `/remove-metadata`, `/podcast-titles`.
- **Library items**: `/items/:id` (GET/DELETE), `/media` (PATCH), `/cover` (GET/POST/PATCH/DELETE), `/match`, `/play`, `/play/:episodeId`, `/tracks`, `/scan`, `/chapters`, `/ffprobe/:fileid`, `/file/:fileid`, `/ebook/:fileid?`, `/ebook/:fileid/status`, plus `/items/batch/{delete,update,get,quickmatch,scan}`.
- **Me**: `/me`, `/me/listening-sessions`, `/me/listening-stats`, `/me/progress/:id/:episodeId?`, `/me/progress/batch/update`, `/me/item/:id/bookmark` (POST/PATCH/DELETE), `/me/password`, `/me/items-in-progress`, `/me/stats/year/:year`, `/me/ereader-devices`.
- **Users**: `/users` CRUD, `/users/online`, `/users/:id/listening-sessions`, `/users/:id/listening-stats`, `/users/:id/openid-unlink`.
- **Collections / Playlists / Series / Authors**: CRUD + batch add/remove, author `match`, author `image`.
- **Podcasts**: `/podcasts` POST, `/podcasts/feed`, `/podcasts/opml/parse`, `/podcasts/opml/create`, `/podcasts/:id/checknew`, `/downloads`, `/clear-queue`, `/search-episode`, `/download-episodes`, `/match-episodes`, `/episode/:episodeId` (GET/PATCH/DELETE).
- **Search (metadata providers)**: `/search/covers`, `/search/books`, `/search/podcast`, `/search/authors`, `/search/chapters`, `/search/providers`.
- **Sessions**: `/sessions` (admin), `/session/:id` open-session sync/close, `/session/local`, `/session/local-all`.
- **Tools**: `/tools/item/:id/encode-m4b`, `/tools/item/:id/embed-metadata`, `/tools/batch/embed-metadata`.
- **Feeds (RSS publish)**: `/feeds`, `/feeds/item/:itemId/open`, `/feeds/collection/:collectionId/open`, `/feeds/series/:seriesId/open`, `/feeds/:id/close`.
- **Admin**: `/notifications*`, `/emails/settings`, `/emails/test`, `/emails/send-ebook-to-device`, `/emails/ereader-devices`, `/cache/purge`, `/cache/items/purge`, `/backups*`, `/api-keys*`, `/custom-metadata-providers*`, `/stats/server`, `/stats/year/:year`, `/share/mediaitem`, `/auth-settings`, `/watcher/update`, `/tags*`, `/genres*`, `/validate-cron`, `/logger-data`, `/settings`, `/sorting-prefixes`.

## HLS router

[server/routers/HlsRouter.js](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/HlsRouter.js) mounts one path: `GET /hls/:stream/:file` (`.m3u8` or `.ts`). Auth is the same JWT middleware at the parent `/hls` mount ([Server.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Server.js)). Missing segments trigger a `SocketAuthority.emitter('stream_reset', …)` so clients seek to the new start.

## Public router

[server/routers/PublicRouter.js](https://github.com/advplyr/audiobookshelf/blob/master/server/routers/PublicRouter.js) serves unauthenticated shares:

- `GET /public/share/:slug` (landing)
- `GET /public/share/:slug/track/:index` (direct-play audio)
- `GET /public/share/:slug/cover`
- `GET /public/share/:slug/download`
- `PATCH /public/share/:slug/progress`
- `GET /public/session/:id/track/:index` (direct-play for an active session)

## Feed delivery

Registered directly on the root router in [Server.js](https://github.com/advplyr/audiobookshelf/blob/master/server/Server.js):

- `GET /feed/:slug` — RSS XML
- `GET /feed/:slug/cover*`
- `GET /feed/:slug/item/:episodeId/*`

## Socket.io events

Two socket.io servers mounted — one at `/socket.io`, one at `${RouterBasePath}/socket.io` for reverse-proxy setups ([SocketAuthority.js](https://github.com/advplyr/audiobookshelf/blob/master/server/SocketAuthority.js)). Client-to-server events: `auth`, `cancel_scan`, `search_covers`, `cancel_cover_search`, `set_log_listener`, `remove_log_listener`, `message_all_users` (admin), `ping`. Server-to-client events (sampled): `init`, `auth_failed`, `admin_message`, `pong`, `stream_reset`, `user_offline`, `user_online`, `item_added`, `item_updated`, `item_removed`, `items_added`, `items_updated`, `author_added`, `author_updated`, `author_removed`, `series_added`, `series_removed`, `collection_added`, `collection_updated`, `collection_removed`, `rss_feed_open`, `rss_feed_closed`, `episode_download_queued`, `episode_download_started`, `episode_download_finished`, `episode_download_queue_cleared`, `metadata_embed_queue_update`, `scan_start`, `scan_complete`.

## No OPDS, no Kobo sync

Unlike Calibre-Web, ABS publishes content-as-podcast-RSS and relies on first-party mobile apps + web player. Mobile apps are first-class consumers of the same `/api/*` surface (not a separate protocol) — the only bespoke mobile endpoints are `/api/session/local*` for offline playback sync and a `X-API-KEY` header pattern.

Omnibus' dual plan — `/api/*` as primary REST contract plus Dioxus server functions at `/api/rpc/*` for the web client — is consistent with ABS's "one REST surface for everything." Mount OPDS and a Kobo-sync compatibility layer on top later; don't build parallel contracts.

---

[← Schema details](5-schema-details.md) · [Next: recommendations →](7-recommendations.md)
