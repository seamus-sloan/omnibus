# 2. Calibre-Web feature inventory

## Browse / search / filter

The home route `/` → `render_books_list("newest", ...)` in [cps/web.py](https://github.com/janeczku/calibre-web/blob/master/cps/web.py) dispatches to category-specific renderers (`rated`, `discover`, `unread`, `read`, `hot`, `downloaded`). Browse-by-\* routes for **author, publisher, series, ratings, formats, category (tags), language** are registered dynamically in a loop. Pagination offset is a plain `int(config_books_per_page) * (page - 1)` passed to `fill_indexpage()` in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py) — no cursor, no keyset; large offsets scan.

Search has two modes. **Simple search** hits `/search?query=` and funnels into `LIKE`/`ILIKE` pattern matching on title/author/publisher/comments. **Advanced search** in [cps/search.py](https://github.com/janeczku/calibre-web/blob/master/cps/search.py) composes filters via helpers (`adv_search_tag`, `adv_search_serie`, `adv_search_language`, etc.) — include/exclude for tags, series, shelves, languages, formats, rating ranges, read/unread, pubdate start/end, plus custom-column filters. There is **no tokenization, stemming, or fuzzy matching** in the Python code itself.

**FTS5 is opt-in and external.** PR [#3476](https://github.com/janeczku/calibre-web/pull/3476) added an FTS5 fast path: it runs `SELECT name FROM sqlite_master WHERE type='table' AND name='books_fts'`, and if that virtual table exists (created externally by Calibre desktop's own full-text index), it issues `SELECT DISTINCT rowid FROM books_fts WHERE books_fts MATCH :term`. Otherwise it falls back to the legacy LIKE path. Calibre-Web **never creates** the FTS table itself.

## Book detail / editing

`show_book` renders detail; `@editbook.route("/admin/book/<int:book_id>", methods=['POST'])` in [cps/editbooks.py](https://github.com/janeczku/calibre-web/blob/master/cps/editbooks.py) accepts edits. Editable fields: title, author/author_sort, series, series_index, publisher, tags, languages, comments (HTML, sanitized with bleach), rating, identifiers (ISBN/ASIN/etc.), per-format conversion, archived flag, read status, and all custom columns. Writes go to SQLAlchemy → `calibre_db.session.commit()` → `helper.update_dir_structure()` renames on-disk folders if author/title changed. OPF regeneration is **deferred** — a `metadata_backup` scheduled task walks the `Metadata_Dirtied` table and writes `metadata.opf` per book.

## Upload & conversion

`@editbook.route("/upload", ["POST"])` accepts `.pdf .epub .kepub .fb2 .cbz .cbr .cbt .cb7 .mp3 .ogg .flac .wav .aac .aiff .asf .mp4 .m4a .m4b .ogv .opus` (see [cps/uploader.py](https://github.com/janeczku/calibre-web/blob/master/cps/uploader.py)). PDF cover extraction uses ImageMagick via Wand. EPUB parsing in [cps/epub.py](https://github.com/janeczku/calibre-web/blob/master/cps/epub.py) uses `lxml` against the OPF (Dublin Core) with Calibre-specific `calibre:series` / `calibre:series_index` XPath. Format conversion shells out to Calibre's `ebook-convert` binary:

```python
[config.config_converterpath, (file_path + format_old_ext), (file_path + format_new_ext)]
```

`kepubify` is called separately for EPUB→KEPUB. See [cps/tasks/convert.py](https://github.com/janeczku/calibre-web/blob/master/cps/tasks/convert.py).

## OPDS

A full OPDS 1.2 catalog from [cps/opds.py](https://github.com/janeczku/calibre-web/blob/master/cps/opds.py): root `/opds`, OpenSearch descriptor `/opds/osd`, `/opds/search`, `/opds/new`, `/opds/discover`, `/opds/rated`, `/opds/hot`, plus letter-indexed author/series/category browses (`/opds/author/letter/<letter>`, `/opds/series/letter/<letter>`), per-entity endpoints, shelf browse, read/unread lists, download, cover, and a JSON `/opds/stats`. **No OPDS 2.0** (JSON) output.

## Kobo sync

[cps/kobo.py](https://github.com/janeczku/calibre-web/blob/master/cps/kobo.py) implements `/kobo/v1/library/sync`, `/kobo/v1/library/<uuid>/metadata`, `/kobo/v1/library/<uuid>/state` (GET/PUT reading state with bookmarks & statistics), `/kobo/v1/library/<uuid>` DELETE (archive), `/kobo/v1/library/tags` (shelves-as-tags, POST/DELETE), `/kobo/v1/library/tags/<id>/items` (add/remove), cover image route, and a book download route. A hard limit of `SYNC_ITEM_LIMIT = 100` caps items per sync response — the main known bottleneck (see issue [#1276](https://github.com/janeczku/calibre-web/issues/1276): libraries of "a few thousand books" time out Kobo Aura H2O after ~286K chars of metadata).

## Email / send-to-kindle

`send_mail(book_id, book_format, convert, ereader_mail, calibrepath, user_id)` in [cps/helper.py](https://github.com/janeczku/calibre-web/blob/master/cps/helper.py) queues a `TaskEmail` on the worker. Conversion to MOBI/EPUB/AZW3 is chained as a `TaskConvert`. Gmail OAuth2 sender is optional via [cps/services/gmail.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/gmail.py). `send_registration_mail` and `send_test_mail` reuse the same worker path.

## In-browser readers

`@web.route("/read/<int:book_id>/<book_format>")` picks a template based on format. Libraries bundled under `cps/static/js/libs/`:

- **EPUB/KEPUB** → epub.js (`epub.min.js` + `jszip_epub.min.js`) in `read.html`.
- **PDF** → pdf.js (Mozilla) in `readpdf.html`.
- **CBR/CBZ** → Kthoom (vendored `kthoom.js`).
- **DJVU** → djvu.js in `readdjvu.html`.
- **TXT** → `readtxt.html`.
- **Audio** → SoundManager 2 + `bar-ui.js` in `listenmp3.html` (HTML5 audio preferred).

## Auth

Local password (Flask-Login fork `cw_login`), **LDAP** via [cps/services/simpleldap.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/simpleldap.py), **OAuth for GitHub and Google only** (hard-coded in `generate_oauth_blueprints` in [cps/oauth_bb.py](https://github.com/janeczku/calibre-web/blob/master/cps/oauth_bb.py)), **reverse-proxy header auth** (any header-supplied user who pre-exists in the DB), and **magic-link remote login** in [cps/remotelogin.py](https://github.com/janeczku/calibre-web/blob/master/cps/remotelogin.py) — the e-reader hits `/remote/login` to mint a token, the user visits `/verify/<token>` in a logged-in browser, the reader polls `/ajax/verify_token`.

## Shelves, read status, bookmarks

Shelves (public/private, reorderable) via [cps/shelf.py](https://github.com/janeczku/calibre-web/blob/master/cps/shelf.py): create, edit, delete, add, remove, mass-add/remove, order. Read status is either a `ub.ReadBook` row or a custom-column bool (configured via `config.config_read_column`). Bookmarks are per-user+book+format key/value (`ub.Bookmark`); Kobo bookmarks are stored separately in `ub.KoboBookmark` with `location_source/type/value` + `progress_percent`.

## Admin

From [cps/admin.py](https://github.com/janeczku/calibre-web/blob/master/cps/admin.py): user table, user new/edit/reset-password, global config, mail settings, scheduled tasks, log viewer + download, debug info, shutdown (restart or stop), metadata backup, full Kobo sync, import LDAP users, cancel task, update status. Permissions are per-user bitflags (upload, edit, download, public-shelf, admin, etc.) plus allowed/denied tag lists and allowed/denied custom-column values for content hiding.

## Metadata providers

Searched in parallel via `ThreadPoolExecutor(max_workers=5)` in [cps/search_metadata.py](https://github.com/janeczku/calibre-web/blob/master/cps/search_metadata.py): **Amazon**, **Google Books**, **ComicVine**, **Douban**, **Google Scholar**, **Lubimyczytać**. Goodreads author info via [cps/services/goodreads_support.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/goodreads_support.py). **Dropbox is not supported** — only Google Drive via PyDrive2.

## Background tasks

[cps/tasks/](https://github.com/janeczku/calibre-web/tree/master/cps/tasks): `convert.py`, `mail.py`, `thumbnail.py`, `metadata_backup.py`, `database.py`, `upload.py`, `clean.py`. Scheduled ones ([cps/schedule.py](https://github.com/janeczku/calibre-web/blob/master/cps/schedule.py)): DB reconnect, thumbnail generation (covers + series), metadata backup, temp-folder cleanup. APScheduler cron-triggered within `config.schedule_start_time` + `config.schedule_duration`.

## Other

- **i18n.** 28 locales in [cps/translations/](https://github.com/janeczku/calibre-web/tree/master/cps/translations). Flask-Babel + `babel.cfg`.
- **Themes.** `config_theme` (light/dark/caliBlur), `config_css_file`, `config_custom_logo`. No full templating theme system.
- **Registration & verification.** `config_public_reg`; optional email verification. Domain allow/deny is its own SQL table (`Registration(id, domain, allow)`).

---

[← Omnibus state](1-omnibus-state.md) · [Next: performance pain points →](3-performance-pain-points.md)
