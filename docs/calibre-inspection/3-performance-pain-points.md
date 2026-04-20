# 3. Performance pain points in Calibre-Web

## Cold browse on large libraries

`fill_indexpage` in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py) does `order_by(*order).offset(off).limit(pagesize)` and a separate `count()`. No eager loading was applied pre-#3476 — each book's author/tag/series backref fired a lazy query in the template. PR [#3476](https://github.com/janeczku/calibre-web/pull/3476) added `selectinload()` on authors and swapped `.any()` subqueries for JOINs; reported 3–9s → 85–330ms on a 129K-book library (89–97% improvement). **Default `config_books_per_page` is 60**, which on the old code meant ~60×(authors+tags+series) trips per page.

## N+1 hotspots that remain

`render_hot_books` queries `Downloads`, then for each row calls `calibre_db.generate_linked_query()`. `show_book` issues sequential queries for read status, archive flag, and shelves. Book-list templates walk `book.authors`/`book.tags`/`book.languages`/`book.publishers` — six many-to-many relationships eagerly flattened in Jinja (Bootstrap 3, server-rendered).

## Cover thumbnails

[cps/tasks/thumbnail.py](https://github.com/janeczku/calibre-web/blob/master/cps/tasks/thumbnail.py) generates three sizes (`COVER_THUMBNAIL_SMALL=1`, `_MEDIUM=2`, `_LARGE=4` in [cps/constants.py](https://github.com/janeczku/calibre-web/blob/master/cps/constants.py); bitmask values). Regeneration triggers: missing resolution, `book.last_modified > thumbnail.generated_at`, or missing file. Generation is **scheduled** (not on-demand during request) — users browsing a freshly imported library before the nightly task has run get the full-size original cover re-scaled by the browser. This is the common "slow covers" complaint (issues [#742](https://github.com/janeczku/calibre-web/issues/742), [#2789](https://github.com/janeczku/calibre-web/issues/2789), [#2817](https://github.com/janeczku/calibre-web/issues/2817)). `clear_cover_thumbnail_cache(book_id)` enqueues invalidation on edit.

## No HTTP-level cache

Issue [#2789](https://github.com/janeczku/calibre-web/issues/2789) proposed an LRU cache for index pages (400ms → 10ms in a PoC) but was **never merged** — cache invalidation was the blocker.

## Kobo sync

`SYNC_ITEM_LIMIT=100` is a hard cap per request, and full metadata is serialized on every initial sync. Issue [#1276](https://github.com/janeczku/calibre-web/issues/1276) describes multi-thousand-book libraries timing out Kobo devices. Kobo cover delivery on-the-fly (route `HandleCoverImageRequest`) used to re-encode each request before the thumbnail cache landed.

## Metadata fetching blocks

`search_metadata` fans out 5 parallel provider requests but waits on `as_completed` before the handler returns — a slow provider lengthens every dialog open.

## Library scan

`helper.update_dir_structure` and OPF writing run inside the `WorkerThread`, which is **a single global thread** (see below). No "watch the directory" daemon; a full metadata re-read is scheduled daily.

## Concurrency model

[cps/server.py](https://github.com/janeczku/calibre-web/blob/master/cps/server.py) prefers **gevent's `WSGIServer` with `spawn=Pool()`**, falling back to **Tornado's `HTTPServer` + `IOLoop`** if gevent isn't installed. No Gunicorn, no multi-process. `WorkerThread` in [cps/services/worker.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/worker.py) is a **single `threading.Thread`** consuming one `queue.Queue` — "conversion + email + thumbnail + metadata-backup all serialize through one thread":

```python
while main_thread.is_alive():
    item = self.queue.get(timeout=1)
    item.task.start(self)
```

A slow `ebook-convert` blocks every other task behind it. GIL is mostly irrelevant because the hot paths (conversion) are subprocess-bound, but covers, metadata parsing, and thumbnails are pure-Python and serialize.

## Python footprint

Pillow/Wand/lxml/cryptography/APScheduler/SQLAlchemy pulls ~70–100 MB RSS at idle on a Pi — frequent complaint in issues. SQLAlchemy 1.3 ORM object creation is also expensive per row.

---

[← Feature inventory](2-feature-inventory.md) · [Next: Dioxus / Rust wins →](4-dioxus-rust-wins.md)
