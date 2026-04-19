# Calibre-Web deep-dive for Omnibus

A source-level study of [janeczku/calibre-web](https://github.com/janeczku/calibre-web) (Flask + SQLAlchemy, Python 3) intended to guide Omnibus' Rust/Dioxus reimplementation. The goal is to reuse everything Calibre-Web got right, avoid its known pain points, and exploit the parts of our stack (axum + sqlx + Dioxus fullstack) that Flask + SQLAlchemy cannot match.

Contents:

1. [Where Omnibus stands today](#1-where-omnibus-stands-today)
2. [Calibre-Web feature inventory](#2-calibre-web-feature-inventory)
3. [Performance pain points in Calibre-Web](#3-performance-pain-points-in-calibre-web)
4. [Where Dioxus / Rust wins](#4-where-dioxus--rust-wins)
5. [Schema details worth copying (and improving)](#5-schema-details-worth-copying-and-improving)
6. [API surface](#6-api-surface)
7. [Recommendations for Omnibus](#7-recommendations-for-omnibus)

---

## 1. Where Omnibus stands today

Single SQLite DB, created inline at startup in [frontend/src/db.rs](../frontend/src/db.rs):

- `books` — one row per file, keyed by `(library_path, filename)`. Dublin Core fields as columns; contributors/identifiers/subjects as **JSON blobs**.
- `book_covers` — BLOBs with FK + ON DELETE CASCADE (manually enforced in `replace_books`).
- `settings`, `library_index_state`, `app_state` (placeholder from the counter demo).

Data flow: [scanner.rs](../frontend/src/scanner.rs) walks the library path → [ebook.rs](../frontend/src/ebook.rs) opens each epub with the `epub` crate and pulls DC metadata + cover bytes → [indexer.rs](../frontend/src/indexer.rs) performs an atomic `replace_books()` every 60 minutes of staleness. The filesystem is **read-only**; the DB is a rebuildable cache.

The gap between this and Calibre-Web is large — but it's the gap we want to close deliberately, not by cloning Calibre-Web's schema mistakes.

---

## 2. Calibre-Web feature inventory

### Browse / search / filter

The home route `/` → `render_books_list("newest", ...)` in [cps/web.py](https://github.com/janeczku/calibre-web/blob/master/cps/web.py) dispatches to category-specific renderers (`rated`, `discover`, `unread`, `read`, `hot`, `downloaded`). Browse-by-\* routes for **author, publisher, series, ratings, formats, category (tags), language** are registered dynamically in a loop. Pagination offset is a plain `int(config_books_per_page) * (page - 1)` passed to `fill_indexpage()` in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py) — no cursor, no keyset; large offsets scan.

Search has two modes. **Simple search** hits `/search?query=` and funnels into `LIKE`/`ILIKE` pattern matching on title/author/publisher/comments. **Advanced search** in [cps/search.py](https://github.com/janeczku/calibre-web/blob/master/cps/search.py) composes filters via helpers (`adv_search_tag`, `adv_search_serie`, `adv_search_language`, etc.) — include/exclude for tags, series, shelves, languages, formats, rating ranges, read/unread, pubdate start/end, plus custom-column filters. There is **no tokenization, stemming, or fuzzy matching** in the Python code itself.

**FTS5 is opt-in and external.** PR [#3476](https://github.com/janeczku/calibre-web/pull/3476) added an FTS5 fast path: it runs `SELECT name FROM sqlite_master WHERE type='table' AND name='books_fts'`, and if that virtual table exists (created externally by Calibre desktop's own full-text index), it issues `SELECT DISTINCT rowid FROM books_fts WHERE books_fts MATCH :term`. Otherwise it falls back to the legacy LIKE path. Calibre-Web **never creates** the FTS table itself.

### Book detail / editing

`show_book` renders detail; `@editbook.route("/admin/book/<int:book_id>", methods=['POST'])` in [cps/editbooks.py](https://github.com/janeczku/calibre-web/blob/master/cps/editbooks.py) accepts edits. Editable fields: title, author/author_sort, series, series_index, publisher, tags, languages, comments (HTML, sanitized with bleach), rating, identifiers (ISBN/ASIN/etc.), per-format conversion, archived flag, read status, and all custom columns. Writes go to SQLAlchemy → `calibre_db.session.commit()` → `helper.update_dir_structure()` renames on-disk folders if author/title changed. OPF regeneration is **deferred** — a `metadata_backup` scheduled task walks the `Metadata_Dirtied` table and writes `metadata.opf` per book.

### Upload & conversion

`@editbook.route("/upload", ["POST"])` accepts `.pdf .epub .kepub .fb2 .cbz .cbr .cbt .cb7 .mp3 .ogg .flac .wav .aac .aiff .asf .mp4 .m4a .m4b .ogv .opus` (see [cps/uploader.py](https://github.com/janeczku/calibre-web/blob/master/cps/uploader.py)). PDF cover extraction uses ImageMagick via Wand. EPUB parsing in [cps/epub.py](https://github.com/janeczku/calibre-web/blob/master/cps/epub.py) uses `lxml` against the OPF (Dublin Core) with Calibre-specific `calibre:series` / `calibre:series_index` XPath. Format conversion shells out to Calibre's `ebook-convert` binary:

```python
[config.config_converterpath, (file_path + format_old_ext), (file_path + format_new_ext)]
```

`kepubify` is called separately for EPUB→KEPUB. See [cps/tasks/convert.py](https://github.com/janeczku/calibre-web/blob/master/cps/tasks/convert.py).

### OPDS

A full OPDS 1.2 catalog from [cps/opds.py](https://github.com/janeczku/calibre-web/blob/master/cps/opds.py): root `/opds`, OpenSearch descriptor `/opds/osd`, `/opds/search`, `/opds/new`, `/opds/discover`, `/opds/rated`, `/opds/hot`, plus letter-indexed author/series/category browses (`/opds/author/letter/<letter>`, `/opds/series/letter/<letter>`), per-entity endpoints, shelf browse, read/unread lists, download, cover, and a JSON `/opds/stats`. **No OPDS 2.0** (JSON) output.

### Kobo sync

[cps/kobo.py](https://github.com/janeczku/calibre-web/blob/master/cps/kobo.py) implements `/kobo/v1/library/sync`, `/kobo/v1/library/<uuid>/metadata`, `/kobo/v1/library/<uuid>/state` (GET/PUT reading state with bookmarks & statistics), `/kobo/v1/library/<uuid>` DELETE (archive), `/kobo/v1/library/tags` (shelves-as-tags, POST/DELETE), `/kobo/v1/library/tags/<id>/items` (add/remove), cover image route, and a book download route. A hard limit of `SYNC_ITEM_LIMIT = 100` caps items per sync response — the main known bottleneck (see issue [#1276](https://github.com/janeczku/calibre-web/issues/1276): libraries of "a few thousand books" time out Kobo Aura H2O after ~286K chars of metadata).

### Email / send-to-kindle

`send_mail(book_id, book_format, convert, ereader_mail, calibrepath, user_id)` in [cps/helper.py](https://github.com/janeczku/calibre-web/blob/master/cps/helper.py) queues a `TaskEmail` on the worker. Conversion to MOBI/EPUB/AZW3 is chained as a `TaskConvert`. Gmail OAuth2 sender is optional via [cps/services/gmail.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/gmail.py). `send_registration_mail` and `send_test_mail` reuse the same worker path.

### In-browser readers

`@web.route("/read/<int:book_id>/<book_format>")` picks a template based on format. Libraries bundled under `cps/static/js/libs/`:

- **EPUB/KEPUB** → epub.js (`epub.min.js` + `jszip_epub.min.js`) in `read.html`.
- **PDF** → pdf.js (Mozilla) in `readpdf.html`.
- **CBR/CBZ** → Kthoom (vendored `kthoom.js`).
- **DJVU** → djvu.js in `readdjvu.html`.
- **TXT** → `readtxt.html`.
- **Audio** → SoundManager 2 + `bar-ui.js` in `listenmp3.html` (HTML5 audio preferred).

### Auth

Local password (Flask-Login fork `cw_login`), **LDAP** via [cps/services/simpleldap.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/simpleldap.py), **OAuth for GitHub and Google only** (hard-coded in `generate_oauth_blueprints` in [cps/oauth_bb.py](https://github.com/janeczku/calibre-web/blob/master/cps/oauth_bb.py)), **reverse-proxy header auth** (any header-supplied user who pre-exists in the DB), and **magic-link remote login** in [cps/remotelogin.py](https://github.com/janeczku/calibre-web/blob/master/cps/remotelogin.py) — the e-reader hits `/remote/login` to mint a token, the user visits `/verify/<token>` in a logged-in browser, the reader polls `/ajax/verify_token`.

### Shelves, read status, bookmarks

Shelves (public/private, reorderable) via [cps/shelf.py](https://github.com/janeczku/calibre-web/blob/master/cps/shelf.py): create, edit, delete, add, remove, mass-add/remove, order. Read status is either a `ub.ReadBook` row or a custom-column bool (configured via `config.config_read_column`). Bookmarks are per-user+book+format key/value (`ub.Bookmark`); Kobo bookmarks are stored separately in `ub.KoboBookmark` with `location_source/type/value` + `progress_percent`.

### Admin

From [cps/admin.py](https://github.com/janeczku/calibre-web/blob/master/cps/admin.py): user table, user new/edit/reset-password, global config, mail settings, scheduled tasks, log viewer + download, debug info, shutdown (restart or stop), metadata backup, full Kobo sync, import LDAP users, cancel task, update status. Permissions are per-user bitflags (upload, edit, download, public-shelf, admin, etc.) plus allowed/denied tag lists and allowed/denied custom-column values for content hiding.

### Metadata providers

Searched in parallel via `ThreadPoolExecutor(max_workers=5)` in [cps/search_metadata.py](https://github.com/janeczku/calibre-web/blob/master/cps/search_metadata.py): **Amazon**, **Google Books**, **ComicVine**, **Douban**, **Google Scholar**, **Lubimyczytać**. Goodreads author info via [cps/services/goodreads_support.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/goodreads_support.py). **Dropbox is not supported** — only Google Drive via PyDrive2.

### Background tasks

[cps/tasks/](https://github.com/janeczku/calibre-web/tree/master/cps/tasks): `convert.py`, `mail.py`, `thumbnail.py`, `metadata_backup.py`, `database.py`, `upload.py`, `clean.py`. Scheduled ones ([cps/schedule.py](https://github.com/janeczku/calibre-web/blob/master/cps/schedule.py)): DB reconnect, thumbnail generation (covers + series), metadata backup, temp-folder cleanup. APScheduler cron-triggered within `config.schedule_start_time` + `config.schedule_duration`.

### Other

- **i18n.** 28 locales in [cps/translations/](https://github.com/janeczku/calibre-web/tree/master/cps/translations). Flask-Babel + `babel.cfg`.
- **Themes.** `config_theme` (light/dark/caliBlur), `config_css_file`, `config_custom_logo`. No full templating theme system.
- **Registration & verification.** `config_public_reg`; optional email verification. Domain allow/deny is its own SQL table (`Registration(id, domain, allow)`).

---

## 3. Performance pain points in Calibre-Web

### Cold browse on large libraries

`fill_indexpage` in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py) does `order_by(*order).offset(off).limit(pagesize)` and a separate `count()`. No eager loading was applied pre-#3476 — each book's author/tag/series backref fired a lazy query in the template. PR [#3476](https://github.com/janeczku/calibre-web/pull/3476) added `selectinload()` on authors and swapped `.any()` subqueries for JOINs; reported 3–9s → 85–330ms on a 129K-book library (89–97% improvement). **Default `config_books_per_page` is 60**, which on the old code meant ~60×(authors+tags+series) trips per page.

### N+1 hotspots that remain

`render_hot_books` queries `Downloads`, then for each row calls `calibre_db.generate_linked_query()`. `show_book` issues sequential queries for read status, archive flag, and shelves. Book-list templates walk `book.authors`/`book.tags`/`book.languages`/`book.publishers` — six many-to-many relationships eagerly flattened in Jinja (Bootstrap 3, server-rendered).

### Cover thumbnails

[cps/tasks/thumbnail.py](https://github.com/janeczku/calibre-web/blob/master/cps/tasks/thumbnail.py) generates three sizes (`COVER_THUMBNAIL_SMALL=1`, `_MEDIUM=2`, `_LARGE=4` in [cps/constants.py](https://github.com/janeczku/calibre-web/blob/master/cps/constants.py); bitmask values). Regeneration triggers: missing resolution, `book.last_modified > thumbnail.generated_at`, or missing file. Generation is **scheduled** (not on-demand during request) — users browsing a freshly imported library before the nightly task has run get the full-size original cover re-scaled by the browser. This is the common "slow covers" complaint (issues [#742](https://github.com/janeczku/calibre-web/issues/742), [#2789](https://github.com/janeczku/calibre-web/issues/2789), [#2817](https://github.com/janeczku/calibre-web/issues/2817)). `clear_cover_thumbnail_cache(book_id)` enqueues invalidation on edit.

### No HTTP-level cache

Issue [#2789](https://github.com/janeczku/calibre-web/issues/2789) proposed an LRU cache for index pages (400ms → 10ms in a PoC) but was **never merged** — cache invalidation was the blocker.

### Kobo sync

`SYNC_ITEM_LIMIT=100` is a hard cap per request, and full metadata is serialized on every initial sync. Issue [#1276](https://github.com/janeczku/calibre-web/issues/1276) describes multi-thousand-book libraries timing out Kobo devices. Kobo cover delivery on-the-fly (route `HandleCoverImageRequest`) used to re-encode each request before the thumbnail cache landed.

### Metadata fetching blocks

`search_metadata` fans out 5 parallel provider requests but waits on `as_completed` before the handler returns — a slow provider lengthens every dialog open.

### Library scan

`helper.update_dir_structure` and OPF writing run inside the `WorkerThread`, which is **a single global thread** (see below). No "watch the directory" daemon; a full metadata re-read is scheduled daily.

### Concurrency model

[cps/server.py](https://github.com/janeczku/calibre-web/blob/master/cps/server.py) prefers **gevent's `WSGIServer` with `spawn=Pool()`**, falling back to **Tornado's `HTTPServer` + `IOLoop`** if gevent isn't installed. No Gunicorn, no multi-process. `WorkerThread` in [cps/services/worker.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/worker.py) is a **single `threading.Thread`** consuming one `queue.Queue` — "conversion + email + thumbnail + metadata-backup all serialize through one thread":

```python
while main_thread.is_alive():
    item = self.queue.get(timeout=1)
    item.task.start(self)
```

A slow `ebook-convert` blocks every other task behind it. GIL is mostly irrelevant because the hot paths (conversion) are subprocess-bound, but covers, metadata parsing, and thumbnails are pure-Python and serialize.

### Python footprint

Pillow/Wand/lxml/cryptography/APScheduler/SQLAlchemy pulls ~70–100 MB RSS at idle on a Pi — frequent complaint in issues. SQLAlchemy 1.3 ORM object creation is also expensive per row.

---

## 4. Where Dioxus / Rust wins

Given Omnibus' stack (axum + sqlx + Dioxus fullstack):

- **sqlx vs SQLAlchemy.** sqlx emits raw SQL with compile-time checks; the JOIN-over-`.any()` optimization PR #3476 had to retrofit is *the default* on sqlx. Collecting into serializable structs (`#[derive(FromRow)]`) skips ORM object graph hydration, which is a large fraction of SQLAlchemy's per-row cost. Expect Omnibus to not need PR #3476's heroics at all.

- **No GIL.** Cover extraction, EPUB OPF parsing, and thumbnail resize (the `image` crate) can truly parallelize across cores during library scan. Calibre-Web's single `WorkerThread` becomes a `tokio::task::JoinSet` bounded by a `Semaphore`. `ebook-convert` is still subprocess-bound — but you can run N of them concurrently without starving the web path.

- **SSR + WASM hydration.** Dioxus fullstack lets you render the book grid server-side (no blank page, good for low-end devices), then hydrate for interactive filter/sort/search. Calibre-Web full-reloads the page for every filter click — Jinja has no concept of "add another tag filter" without a new HTTP round trip. Dioxus signals can re-filter a pre-fetched `Vec<BookListItem>` in-memory without touching the network.

- **Embedded FTS5, always.** Create the `books_fts` virtual table at startup in `initialize_schema` with triggers on `books` (insert/update/delete) so the index stays in sync. Calibre-Web only gets FTS5 when Calibre-desktop created it; Omnibus can guarantee it, so the fast path is the only path.

- **Streaming responses.** axum's `StreamBody` + server-sent events or chunked JSON can deliver the first 50 books while the next 50 are still being assembled — useful for a Kobo-like sync endpoint that wants to respect a client's read buffer instead of the hard `SYNC_ITEM_LIMIT=100` cutoff.

- **Native image processing.** Swap Wand+ImageMagick for the `image` crate + `resvg` (for SVG covers). Deterministic binary, no shell out, no ImageMagick policy file. PDF first-page extraction can use `pdfium-render` or `mupdf` bindings for speed.

- **Memory & startup.** A cold cargo-built binary is ~20–40 MB RSS vs 70–100 MB for the Python stack — matters on Pi / Synology deployments that Calibre-Web targets.

- **Prefetch + Dioxus Router.** Link prefetch on hover + route-level code splitting gives sub-100ms navigations between library sections once hydrated, which Calibre-Web cannot do at all.

---

## 5. Schema details worth copying (and improving)

### Calibre side — verbatim from [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py)

```python
class Books(Base):
    __tablename__ = 'books'
    DEFAULT_PUBDATE = datetime(101, 1, 1, 0, 0, 0, 0)
    id = Column(Integer, primary_key=True, autoincrement=True)
    title = Column(String(collation='NOCASE'), nullable=False, default='Unknown')
    sort = Column(String(collation='NOCASE'))
    author_sort = Column(String(collation='NOCASE'))
    timestamp = Column(TIMESTAMP, default=lambda: datetime.now(timezone.utc))
    pubdate = Column(TIMESTAMP, default=DEFAULT_PUBDATE)
    series_index = Column(String, nullable=False, default="1.0")   # stored as TEXT!
    last_modified = Column(TIMESTAMP, default=lambda: datetime.now(timezone.utc))
    path = Column(String, default="", nullable=False)
    has_cover = Column(Integer, default=0)
    uuid = Column(String)
```

```python
class Data(Base):
    __tablename__ = 'data'
    id = Column(Integer, primary_key=True)
    book = Column(Integer, ForeignKey('books.id'), nullable=False)
    format = Column(String(collation='NOCASE'), nullable=False)
    uncompressed_size = Column(Integer, nullable=False)
    name = Column(String, nullable=False)   # filename stem, no extension
```

### On-disk path construction

From `convert_book_format` in [cps/helper.py](https://github.com/janeczku/calibre-web/blob/master/cps/helper.py):

```python
file_path = os.path.join(calibre_path, book.path, data.name + "." + book_format.lower())
```

That is: `<library_root>/<books.path>/<data.name>.<format-lowercased>`. `books.path` is typically `Author/Title (id)`; `data.name` duplicates that title fragment without extension. Omnibus should keep this convention for Calibre interop but normalize internally — `data.name` is redundant with `books.path` 99% of the time. Store the extension in a separate column rather than a format-join.

### Collations and indices

Calibre uses SQLite `NOCASE` collation on every string column you'd ever search. sqlx doesn't expose collation declaratively — declare columns with `COLLATE NOCASE` in the CREATE TABLE string.

**Note: no explicit `Index()` declarations** anywhere in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py). Calibre-desktop adds some indices (`idx_books_timestamp`, `authors_idx`) directly on disk but Calibre-Web ORM-side declarations don't. Omnibus should add indices on `(timestamp)`, `(last_modified)`, `(sort)`, `(author_sort)`, `(series_index)` plus each link table's reverse column. An index on `books.uuid` is also needed for Kobo.

### Link tables

Six simple m2m (`books_authors_link`, `books_tags_link`, `books_series_link`, `books_ratings_link`, `books_languages_link`, `books_publishers_link`), all with compound primary keys `(book, <entity>)` and no other columns.

### `custom_columns` table

Row shape: `(id, label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized)`. At startup, Calibre-Web executes `SELECT id, datatype FROM custom_columns` and dynamically **materializes** a SQLAlchemy class per row:

```python
cc = conn.execute(text("SELECT id, datatype FROM custom_columns"))
cls.setup_db_cc_classes(cc)
setattr(Books, 'custom_column_' + str(cc_id[0]), relationship(...))
```

The backing tables are `custom_column_N` (and `books_custom_column_N_link` for multi-valued ones). Omnibus likely wants a single `custom_columns`/`custom_column_values` table with a `datatype` discriminator rather than DDL-per-column — Calibre's approach is an artifact of the desktop app predating JSON/discriminated storage.

### `app.db` (user/shelf side) — [cps/ub.py](https://github.com/janeczku/calibre-web/blob/master/cps/ub.py)

```python
class User(UserBase, Base):
    __tablename__ = 'user'
    id = Column(Integer, primary_key=True)
    name = Column(String(64), unique=True)
    email = Column(String(120), unique=True, default="")
    role = Column(SmallInteger, default=constants.ROLE_USER)   # bitmask
    password = Column(String)
    kindle_mail = Column(String(120), default="")
    locale = Column(String(2), default="en")
    sidebar_view = Column(Integer, default=1)                  # bitmask
    default_language = Column(String(3), default="all")
    denied_tags = Column(String, default="")                   # comma-separated
    allowed_tags = Column(String, default="")
    denied_column_value = Column(String, default="")
    allowed_column_value = Column(String, default="")
    view_settings = Column(JSON, default={})
    kobo_only_shelves_sync = Column(Integer, default=0)
```

Other tables: `Shelf`, `ReadBook`, `Bookmark`, `KoboReadingState`, `KoboStatistics`, `KoboBookmark`, `KoboSyncedBooks`, `ArchivedBook`, `Downloads`, `RemoteAuthToken`, `Registration`, `User_Sessions`.

### "Migrations" = runtime ALTER TABLE

[cps/config_sql.py](https://github.com/janeczku/calibre-web/blob/master/cps/config_sql.py) does **not use Alembic**. `_migrate_table(session, orm_class, secret_key=None)` iterates each ORM column, tries a `SELECT`, catches `OperationalError`, and synthesizes `ALTER TABLE ADD COLUMN` with defaults. No version table, no down-migrations. For Omnibus, keep the `initialize_schema` inline approach today but plan for `refinery`/`sqlx-migrate` once the schema stabilizes.

---

## 6. API surface

- **OPDS 1.2** — 30+ endpoints under `/opds`. Atom XML. OpenSearch discovery at `/opds/osd`.
- **Kobo** — under `/kobo/v1/`: `library/sync`, `library/<uuid>/metadata`, `library/<uuid>/state`, `library/<uuid>` (DELETE), `library/tags`, `library/tags/<id>`, `library/tags/<id>/items`, `library/tags/<id>/items/delete`, `<uuid>/<w>/<h>/<grey>/image.jpg`, `download/<book_id>/<format>`. Auth wrapper `@requires_kobo_auth`. Sync token maintenance in [cps/services/SyncToken.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/SyncToken.py).
- **JSON / AJAX** — there is **no general-purpose REST API**. Ad-hoc JSON endpoints are scattered: `/ajax/book/<uuid>` (Calibre Companion), `/ajax/listusers`, `/ajax/deleteuser`, `/ajax/log/<type>`, `/ajax/canceltask`, `/ajax/fullsync/<userid>`, `/ajax/verify_token`, `/opds/stats`. No single documented contract for a mobile client.

Omnibus' plan to expose `/api/*` as a first-class hand-written REST router (see [server/src/backend.rs](../server/src/backend.rs)) is a real improvement; build it as the primary surface and layer OPDS + Kobo on top.

---

## 7. Recommendations for Omnibus

Ordered roughly by payoff vs. cost.

1. **Split `books` from `book_files`.** A book is a logical work; files are its formats. The current one-row-per-file schema blocks the epub+m4b+pdf-for-one-book case cleanly. Mirror Calibre's `data` table: `book_files(id, book_id, format, filename, size_bytes, mtime)`.

2. **Normalize authors / series / tags / publishers / languages into tables + m2m link tables.** Ship indices on both sides of every link. This is the prerequisite for ROADMAP #3 (Libraries / metadata filters), #4 (Search), and efficient browse-by-\* routes.

3. **Create FTS5 unconditionally at startup.** `CREATE VIRTUAL TABLE books_fts USING fts5(title, authors, series, tags, description, content='books', content_rowid='id')` + AFTER INSERT/UPDATE/DELETE triggers. Avoids Calibre-Web's opt-in split and gives bm25-ranked search without shelling out. Tokenizer: `unicode61 remove_diacritics 2`.

4. **Single SQLite file with real FKs.** Keep users, shelves, read-state, reading progress in the same DB as books. Avoids Calibre-Web's cross-DB orphan bug (`book_shelf.book_id` dangles into `metadata.db` with no FK).

5. **Eager-load relationships the sqlx way.** Build list pages with a single query that joins `books`, aggregates authors/tags as `GROUP_CONCAT`, and deserializes into a flat DTO. Never iterate `book.authors` in a render path.

6. **On-demand thumbnail pipeline, not scheduled.** On first cover request at size N, generate, cache to `<data_dir>/thumbs/<book_id>_<n>.webp`, serve. Invalidate by `book.last_modified`. Use `image` + `webp` crates; WebP gives ~30% smaller than JPEG at equivalent quality. Dioxus can emit `srcset` with three sizes so clients pick the right one — Calibre-Web does not.

7. **Keep the DB as cache, filesystem as truth.** Omnibus already does this; preserve it. Means users can hand Omnibus an existing Calibre library and get a rebuildable index without write permission. When/if editing arrives, store overrides in DB rather than rewriting folders — avoids Calibre-Web's racy folder-rename-on-edit path.

8. **OPF/epub-internal metadata = read-only input.** Pick one source of truth. Calibre-Web does partial OPF round-tripping and it's a known drift source. Only write OPF as an export artifact, never as a source-of-truth sync target.

9. **Avoid Calibre-Web's role bitmask.** Use explicit boolean columns or an enum for permissions (`can_upload`, `can_edit`, `is_admin`, `can_download`). Migrations are easier; so is filtering in UI.

10. **Avoid dynamic `custom_column_N` tables.** Use a single `custom_column_values(column_id, book_id, value_text, value_num, value_date)` EAV table or a `custom_metadata JSONB` column on `books`. Calibre's approach is a 2008-era ORM workaround, not a design goal.

11. **Worker = `tokio::task::JoinSet` + `Semaphore`, not a single thread.** Parallelize conversion, thumbnail generation, metadata fetching. A single `ebook-convert` subprocess still takes one core, but five of them can run on a modern NAS without blocking the web path.

12. **Dioxus-specific: signal-driven filter/sort on hydrated lists.** Fetch the book list once server-side, hydrate as a `Signal<Vec<BookListItem>>`, do filter/sort in-memory. Full reload only when paging through >10k books. This is the single biggest user-facing perceived-speed win over Calibre-Web.

13. **Streaming OPDS + Kobo sync.** Remove the 100-item cap; chunk the response so Kobo devices receive and parse progressively. Use `axum::body::StreamBody` with `serde_json::to_writer` per entry.

14. **Design the `/api/*` surface as primary.** A single documented REST contract that OPDS and Kobo wrap. Mobile app already hits `/api/*` (see `mobile/src/main.rs`) — lean into it.

### Cross-reference to ROADMAP

| ROADMAP feature | Recommendations above |
|---|---|
| #1 Book Scanning | 1, 2, 6, 11 |
| #3 Libraries (filters) | 2, 5 |
| #4 Search | 3, 5 |
| #6 Ratings / journaling | 4 |
| #7 Auth | 4, 9 |
| #8 Epub reader | 12 |
| #9 Audiobooks | 1, 2, 11 |
| #10 OPDS | 13, 14 |
| #11 Kobo sync (implied) | 4, 13, 14 |

---

*Unconfirmed claims (needed further source-reading that was out of scope): the exact byte layout of Calibre-desktop's `books_fts` virtual table; whether PR #3476's `selectinload` landed on stable releases or only `master`; the precise path the `FileSystem` cache helper resolves to at runtime.*
