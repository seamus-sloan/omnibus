## Database & Data Layer Review

### Overall Technical Score: 62/100

> Schema design 30 → 16: durable user data (progress, sessions, bookmarks, highlights) cascade-deletes on an unstable `book_id`, and the identity anchor (`stable_uuid`) is path-derived, so several High findings hit the schema's correctness core. Query efficiency 25 → 14: the landing projection and author/series browse are O(rows × subqueries) with no pagination and a quadratic author-detail predicate. Indexing 15 → 11: session tables lack the windowed indexes the stats feature needs, plus four redundant and two speculative indexes. Naming & clarity 15 → 10: the `libraries` name collides with a planned feature, three timestamp encodings coexist, and several dead/vestigial columns linger. Future-fit 15 → 11: no change-tracking for Phase-4 device sync, and the schema diverges from the roadmap's own soft-reference mandate.

---

### High Priority — User data cascade-deletes on reindex

All five user-data tables key on the numeric `book_id` with `ON DELETE CASCADE` to `books(id)`: `reading_progress`, `bookmarks`, `reading_sessions`, `listening_sessions` (`0013_reading_progress.sql:15,39,52,65`) and `highlights` (`0017_highlights.sql:10`). `books.id` is not stable — `books.library_id` itself cascades from `libraries` (`0002_normalized_schema.sql:26`), changing a configured path runs `prune_orphan_libraries` with an explicit `DELETE FROM books WHERE library_id IN (...)` (`db/src/settings.rs:147-152`), and a reindex's Removed bucket hard-DELETEs rows (`db/src/sync/books.rs:154-160`). Any of these paths silently destroys every user's reading position, session history, bookmarks, and highlights, with no way to re-link.

The roadmap forbids exactly this. `docs/roadmap/3-2-ratings-journaling.md:30`: "Do not use `book_id` (INT FK with `ON DELETE CASCADE`) for user data. Cascade-delete is appropriate for ephemeral derived data (covers, FTS index) but not for user-generated data." It prescribes a `book_uuid TEXT NOT NULL` soft reference (no FK, no cascade) so a pruned book's row detaches and auto-relinks when the book reappears. `metadata_overrides` already follows this correctly (`book_uuid` PK, no FK — `0007_metadata_overrides.sql:16-22`), so the schema is internally inconsistent.

F3.2 ratings/journaling and F3.4 stats (which reads `reading_sessions`) build directly on these tables, so this is expensive to reverse once real data exists. Fix direction: migrate the five tables to a `book_uuid TEXT` soft reference, drop the cascade, and add a reindex reconcile/relink step before F2.1 progress sync graduates from schema-only to populated. Co-dependent with the `stable_uuid` finding: even uuid-keyed rows detach today on a library-root change.

---

### High Priority — Path-based stable_uuid breaks durability

`stable_uuid` derives a book's identity from `format!("{library_path}\0{filename}")` via UUIDv5 (`db/src/helpers.rs:29-37`). Changing the library root — a settings edit, the F0.6 filesystem reorg, or moving the data dir — produces a brand-new uuid for every book. Everything keyed on `books.uuid` then silently detaches: `metadata_overrides` (`book_uuid` PK, `0007_metadata_overrides.sql:17`), `merged_uuids` (`0016_format_merge.sql:36-41`), cover filenames / `/api/covers/{uuid}` URLs, and — once F3.2 lands its uuid-soft-ref tables — every rating and journal entry.

The roadmap calls this a hard blocker. `docs/roadmap/3-2-ratings-journaling.md:46-57`: "The current `stable_uuid(library_path, filename)` scheme breaks this... `stable_uuid` must be replaced [with a `dc:identifier`-anchored / content-hash scheme]" and lists it as a dependency that must land before the feature ships. The OPF `dc:identifier` is already parsed during indexing, so the primary anchor is available today; only the derivation and a one-time re-key migration are missing.

This compounds the user-data-cascade finding: even after switching user data to uuid soft-refs, a path change still orphans every row because the uuid itself moves. The two fixes are co-dependent. Because the uuid is embedded in cover filenames, route URLs, and `merged_uuids`, reversing a bad scheme after data accumulates is costly. Fix direction: re-anchor `stable_uuid` on `dc:identifier` with a SHA-256 file-content fallback, plus a re-key migration and a reconciliation pass that re-links detached rows by (author, title). Consumed at `db/src/sync/books.rs:583,320` and resolved at `db/src/books/get.rs:132-145`.

---

### High Priority — `libraries` table name collides with F3.1

The schema's `libraries` table is the physical scan-directory registry: `(id, path, display_name, last_indexed)`, one row per on-disk root, with `books.library_id` FK and `ON DELETE CASCADE` (`0002_normalized_schema.sql:15-20,26`). It is a degenerate 1-or-2-row table driven by the settings KV keys (`db/src/settings.rs:16-17,211`), and every read path actually scopes by `libraries.path` string match (`l.path IN (...)`, `db/src/books/list.rs:69-70`; `db/src/browse.rs:53`), not by `library_id`.

But F3.1 ([`docs/roadmap/3-1-shelves.md`](../roadmap/3-1-shelves.md)) defines a different, user-facing concept once also called "libraries" — named saved metadata-filter collections: a parent table + a `(field, op, value)` rule child table, described as "the core v1 differentiator (v1 #3)." The cascade semantics differ fundamentally: deleting a scan root must prune its books; deleting a saved filter-collection must delete nothing but rule rows. The two cannot share the name.

> **Update (F3.1 redesign):** the user-facing concept was renamed from "Libraries" to **Shelves** and now uses `shelves` / `shelf_rules` tables, so it no longer claims the `libraries` name — this collision is resolved by naming. The recommendation below to give the physical-roots table an unambiguous name (`scan_roots` / `library_sources`) still stands on its own merits.

With F3.1 renamed to use `shelves` tables, the forced collision is gone — but `libraries` is still a confusing name for a degenerate scan-root registry. Fix direction (now a clarity cleanup, not urgent): rename the physical-roots table (e.g. `scan_roots` / `library_sources`) while it has 1-2 rows and the blast radius is internal — touching `books.library_id` and every JOIN in `books/list.rs`, `browse.rs`, discovery, and settings prune.

---

### High Priority — books_fts maintained by hand across writes

`books_fts` is a standalone FTS5 vtable (no `content=`) with only three rename triggers (author/tag/series name UPDATE) and no insert/delete/content triggers (`0005_fts5.sql:22-82`). Consistency depends entirely on every code path that touches `books` also mirroring `books_fts` by hand, in lockstep. There are at least eight such sites: `sync/books.rs` (new `:286`, changed delete+reinsert `:255-262`, removed `:144-152`), `sync/audiobooks.rs` (`:55,144,495`), `settings.rs` prune (`:138`), `merge/transaction.rs` (`:56,117`), `merge/undo.rs` (`:75,92` — post-commit best-effort, logged and swallowed), `metadata_overrides/fts.rs` (rebuild helper), and `author_photos_data.rs:206` (post-commit best-effort). A failure in any post-commit rebuild leaves `books_fts` permanently drifted from `books` with no repair job and no test catch.

There is a concrete latent gap: `attach_ebook_file` (`db/src/sync/books.rs:350-385`) adds a `book_files` row and unions the attached file's identifiers into an existing book (incl. ISBN, `:379`) but never refreshes that book's FTS row — so a newly-attached format's ISBN isn't searchable until the book is next Changed. Minor today but illustrative.

The module doc justifies standalone FTS to avoid per-row trigger fan-out during bulk reindex (`0005_fts5.sql:14-20`), a reasonable tradeoff. But uploads, Kobo state, and any future bulk metadata edit each add a write path that must remember the FTS twin. Fix direction: route every book mutation through a single `upsert_fts(book_id)`/`delete_fts(book_id)` choke-point (the `rebuild_fts_for_books_batch` helper already exists — make it the only door), fix the attach path to call it, add an admin "rebuild search index" job, and add a test asserting no `books` row lacks an FTS row after each public write API.

---

### High Priority — Landing query: 50k rows, no pagination

`BOOK_COLUMNS` (`db/src/books/projection.rs:30-87`) is the shared projection for `list_books` / `get_book` / `search_books` / discovery. It embeds ten correlated scalar subqueries per book row, and `list_books_for_paths` runs this across an entire library capped at `MAX_BOOKS_RETURNED = 50,000` (`projection.rs:24`) with `ORDER BY b.sort, b.id LIMIT 50000` and no SQL keyset pagination — the comment at `projection.rs:23` says cursor pagination is "intentionally deferred", so the whole library is materialized and JSON-encoded on every landing/search load.

Two subquery pairs are pure waste: `primary_filename`/`primary_format` (`projection.rs:35-43`) scan `book_files` twice for the same row to pull two columns, and `series_name`/`series_link_id` (`:55-63`) scan `books_series_link` twice — a single `json_object` select (the pattern already used for creators) would halve them. The landing grid then does two additional full passes over the result in Rust: `merge_overrides_into_books` (`projection.rs:255`) and `backfill_creator_ids` (`:200`), so one render is the big SELECT plus two follow-up round-trips (`db/src/books/list.rs:47-48`).

`ORDER BY b.sort, b.id` has no supporting library-scoped composite index (`idx_books_sort` is global, so it can't both filter by library and provide the sort), forcing a temp-sort. This works at current sizes but is the join-fan-out shape that won't scale, and F3.1 smart shelves will wrap this projection in more predicates. Fix direction: collapse the duplicated subquery pairs into `json_object` selects; add a `(library_id, sort, id)` index for the sort; add real keyset pagination before libraries grow; consider a denormalized book-card read-model materialized by the indexer for the hot path.

---

### High Priority — Author/series browse index is quadratic

`list_authors` and `list_series` compute each row's `book_count` with a correlated scalar subquery (capped at `INDEX_LIMIT=10,000`) that JOINs `books × libraries × metadata_overrides` and, in the non-override branch, runs a per-book EXISTS against the link table — once per author/series. They add further per-row correlated subqueries: an accent subquery joining `books_authors_link → books → libraries` with `ORDER BY + LIMIT`, an EXISTS for `has_photo`, and a top-level EXISTS filter (`db/src/browse.rs:48-92,147-200`). The same `json_each`-over-overrides predicate is re-run in the discovery-detail count (`db/src/discovery/authors.rs:66-80,131-134`). On a multi-thousand-author / multi-thousand-book library this is quadratic JSON-parsing work for one cold `/authors` or `/series` load.

The author-detail path is a separate, fixable outlier: `EFFECTIVE_AUTHOR_PREDICATE` (`db/src/discovery/authors.rs:66`) is anchored on `FROM books b` with a correlated `EXISTS(SELECT 1 FROM books_authors_link bal WHERE bal.book=b.id AND bal.author=?)`, so the planner scans the entire `books` table and tests EXISTS per book instead of seeking through `idx_books_authors_author` from the link side. The series-detail CTE is written correctly — its arm drives through `books_series_link bsl WHERE bsl.series = ?1` (`db/src/discovery/series.rs:73-76`) — which proves the author shape is the avoidable one.

`/authors` and `/series` are core Phase-1 browse pages, and stats targets <250ms cold loads. Fix direction: compute the per-author/series aggregates in a single GROUP BY pass driven by the reverse index (UNION the override-creators case as a separate arm rather than a per-row CASE/EXISTS), and rewrite `EFFECTIVE_AUTHOR_PREDICATE` to start from `books_authors_link WHERE author=?`, matching the series CTE. Consider a denormalized per-author/series summary refreshed at index time.

---

### Medium Priority — book_identifiers PK collapses values per scheme

`book_identifiers` has `PRIMARY KEY(book_id, scheme)` (`0002_normalized_schema.sql:68-73`), so a book can hold at most one value per identifier scheme. The write path enforces this lossily: `insert_identifier_links` uses `INSERT OR REPLACE` keeping the last value per (book_id, scheme) (`db/src/sync/books.rs:752`, comment `:783-784`), and `books.isbn` is filled from only the first ISBN via `first_isbn()` (`db/src/sync/books.rs:473-482,261`). A book legitimately carrying two ISBNs (separate ISBN-10 and ISBN-13, or print vs ebook editions in one OPF) silently loses one. The schema comment frames this as intentional dedup (`0002:66` "duplicates like two ISBNs collapse") but two distinct ISBNs are not duplicates.

This directly undercuts the roadmap's own future matching: F5.10 auto-merge tier 3 is "if both carry ISBNs that resolve to the same Open Library work, auto-merge" (`docs/roadmap/5-10-format-merge.md:98`), and Kobo/metadata enrichment key off the full identifier set — throwing away an ISBN at index time kills that signal before any matcher sees it.

Fix direction: change the PK to `(book_id, scheme, value)` and switch the writer from `INSERT OR REPLACE` to `INSERT OR IGNORE` (the attach path already uses the IGNORE variant, `db/src/sync/books.rs:758-764`). The read projection already aggregates identifiers via `json_group_array` ordered by scheme,value (`db/src/books/projection.rs:78-81`), so it tolerates multiple rows per scheme unchanged. Cheap mechanical migration now; a behavior change with backfill implications after identifier-matching ships.

---

### Medium Priority — books.isbn denormalizes book_identifiers

`books.isbn` (`0002_normalized_schema.sql:37`) is a denormalized copy of the first ISBN-scheme value already stored in `book_identifiers`. It is written redundantly via `first_isbn()` into both `books.isbn` and the `books_fts.isbn` column on every book write (`db/src/sync/books.rs:261,501,520,592,612,824`), while the canonical multi-value list lives in `book_identifiers`. That is three copies of the same fact (`book_identifiers`, `books.isbn`, `books_fts.isbn`) kept in lockstep by hand, and `books.isbn` structurally can never represent ISBN-10+ISBN-13 (coupled to the `book_identifiers` PK finding).

`books.isbn` exists mainly to feed FTS and as a convenience column, but the read projection already reconstructs the full identifier list from `book_identifiers` as JSON (`db/src/books/projection.rs:78-81`), so `books.isbn` is redundant for reads. Fix direction: drop `books.isbn` and source the FTS isbn token directly from `book_identifiers` during the (already manual) FTS write, removing one copy and one drift surface. If a fast single-ISBN lookup is genuinely needed later, make it a view over `book_identifiers`. Lower-risk than the multi-ISBN fix but should travel with it.

---

### Medium Priority — record_session ignores merged_uuids, drops sessions

`record_session_tx` resolves the book with a bare `SELECT id FROM books WHERE uuid = ?` and returns `Ok(false)` (skip) when the uuid is unknown (`db/src/progress.rs:150-156`). But the canonical resolver `resolve_book_id_by_uuid` — used by `upsert_progress` (`db/src/progress.rs:63`) and the cover/thumb/mobile routes — also UNIONs `merged_uuids` so a uuid that was format-merged or auto-attached into another book still resolves (`db/src/books/get.rs:132-145`).

Because session recording does not use that fallback, a client that posts a batched session report for a book whose file was merged after the session started has its session silently dropped (`Ok(false)`) — lost reading/listening minutes that feed the Phase-3 stats feature, with no error surfaced. Progress upserts (which use the fallback) and session inserts (which don't) disagree on identity for the exact same uuid. The two paths drift apart even though they operate on the same `book_uuid` from the same client batch.

Fix direction: route `record_session_tx` through a tx-aware variant of `resolve_book_id_by_uuid` so the two paths agree on book identity. The merged-uuid union is already written and tested for the progress path, so this is a small refactor to share one resolver rather than maintain two divergent lookups.

---

### Medium Priority — fileless `books` rows accumulate with no GC (resolved)

Post-F2 a removed file no longer deletes its `books` row: `mark_book_files_missing` (`db/src/sync/books/shared.rs`) retains the row fileless — its `uuid`, `metadata_overrides`, and soft-ref user data survive — so a returning file re-links by `scan_key`, and library removal is never-prune (`prune_orphan_libraries`). So `metadata_overrides` rows essentially never orphan (only a merge hard-deletes a book, already handled at `db/src/merge/transaction.rs`). The real accumulation is **fileless `books` rows**: `sync_removed` logged the count and deferred their GC to F10.

**Resolved** ([db-review-f10-override-gc.md](../design/db-review-f10-override-gc.md)): a **missing-files GC**. Migration 0029 adds `is_missing_files` / `missing_files_since` / `is_missing_files_override` to `books`; `mark_book_files_missing` drops only `book_files` (keeping the book's links/FTS, so a fileless book still appears in author/series/tag browse + search — the grid/facets hide it via their `EXISTS book_files` filter) and stamps the flag; the file-write chokepoints clear it. `missing_files::gc_books_missing_files`, run best-effort after each reindex, hard-deletes books missing past a 30-day retention that carry **no** user data (any 0027 soft-ref table), no `merged_uuids` attachment, and no wishlist override — taking their regenerable `metadata_overrides` row, `books_fts` twin, cover files, and any now-orphan taxonomy (`taxonomy::delete_orphan_taxonomy`, also run after a merge) with them. Books a user interacted with are kept indefinitely; the "unlinked" admin UX stays deferred to F3.2.

---

### Medium Priority — Three inconsistent time representations

Timestamp columns use three encodings. INTEGER unix-seconds via `strftime('%s','now')`: all auth tables (`0004_auth.sql:28-29,42-43,58-59`), `reading_progress`/`bookmarks`/`reading_sessions`/`listening_sessions` (`0013_reading_progress.sql:19,42,53`), `highlights` (`0017_highlights.sql:15`), `libraries.last_indexed` (`0002:19`). TEXT via `datetime('now')`: `books.timestamp`/`last_modified` (`0002:33-34`), `metadata_overrides.updated_at` (`0007:21`), `author_photos.fetched_at` (`0008:18`), `merge_log.merged_at`/`undone_at` (`0016:53-54`). And a third spelling, DATETIME via `CURRENT_TIMESTAMP`, in `ignored_authors.ignored_at` (`0010:18`).

Mostly cosmetic today because each column is read by code that knows its own format, but it bites two upcoming features. F3.4 stats (`docs/roadmap/3-4-stats.md:18`) aggregates the INTEGER session tables with `GROUP BY date(start_at)` and `SUM(end_at - start_at)` — fine within those tables, but any join/sort against `books.timestamp` (TEXT, "date added") or a future `journal.created_at` into the same timeline can't `GROUP BY date(...)` uniformly: INTEGER columns need `date(col,'unixepoch')`, TEXT ones bare `date(col)`. F3.2 also specs more TEXT `updated_at` columns adjacent to the INTEGER session tables. Mixed types also defeat a single Rust-side serialization helper, and `ORDER BY timestamp` on books is a lexicographic string sort that only works because `datetime('now')` is zero-padded ISO.

The INTEGER-seconds convention is the better one (the auth migration argues this and the newer tables use it). Fix direction: standardize on INTEGER unix-seconds for all machine timestamps and migrate the handful of TEXT/DATETIME columns; keep TEXT only for genuinely partial human dates like `books.pubdate`.

---

### Medium Priority — Session indexes don't support stats

`reading_sessions` and `listening_sessions` are indexed only on `(user_id, book_id)` (`0013_reading_progress.sql:58-59,71-72`). The Phase-3 stats feature aggregates them by time window per user: `GROUP BY date(start_at)`, `SUM(end_at - start_at)` over a date range, targeting cold-load < 250 ms on 10k events (`docs/roadmap/3-4-stats.md:18,32`). With only `(user_id, book_id)` indexes there is no `(user_id, started_at)` index to range-scan a window, so every `/stats` query degrades to scanning the session table filtered in memory by user and date.

The stats roadmap promises "No new schema beyond what F2.1 lands" (`docs/roadmap/3-4-stats.md:21`), so these indexes need to exist now for that promise to hold. Fix direction: add `(user_id, started_at)` indexes to both session tables (and likely `(user_id, ended_at)`); `reading_progress` similarly lacks a `(user_id, updated_at)` index for any "continue reading" rail. Note the actual columns are `started_at`/`ended_at`/`seconds_read` (not the `start_at`/`end_at` the roadmap prose uses, `0013:53-54,66-67`), but the windowed-index need is identical.

---

### Medium Priority — No change-tracking for Phase-4 device sync

Phase-4 device sync (Kobo F4.1, OPDS) needs per-device incremental cursors and a server-side change feed: `/kobo/v1/library/sync` returns only books changed since the client's last token, and Kobo needs delete notifications to remove books from the device. The schema has no monotonic change sequence on books, no per-row `deleted_at` tombstone, and no `kobo_sync_tokens` table (F4.1 names it explicitly, `docs/roadmap/4-1-kobo-sync.md`). `books.last_modified` is TEXT `datetime('now')` at 1-second resolution, overwritten in place on Changed (`db/src/sync/books.rs:509`) — usable as a coarse high-water mark but not a gap-free cursor, and it records no deletions: a removed book is hard-DELETEd via cascade (`db/src/sync/books.rs:154-160`) and simply vanishes, so the sync endpoint cannot tell a client "this book is gone."

This is a data-model gap, not just a missing endpoint. Retrofitting tombstones + a change-sequence onto a schema that hard-deletes rows means either a soft-delete column on books (and teaching every read path to filter it) or a `book_changes` audit table populated by the sync writers — both touch the hot indexer write path. F4.1's listed schema dependency is only the uuid index (already satisfied by the UNIQUE), which undersells the need. Fix direction: decide the change-tracking model (soft-delete + `change_seq` on books, or a `book_changes` feed) in the next schema pass and have `sync_books` / `sync_removed` write it, so F4.1 and OPDS-incremental are cheap to deliver rather than a later refactor.

---

### Medium Priority — Single series_index despite many-to-many

`books_series_link` is a real many-to-many table — `PRIMARY KEY(book, series)` with no `UNIQUE(book)` (`0002_normalized_schema.sql:61`) — so a book can link to multiple series. But the position-within-series number lives as a single scalar `books.series_index REAL` on the books row (`0002:31`), not on the link row. A book that is #1 in one series and #4 in another cannot represent both positions, and the read paths already pick a series arbitrarily (`BOOK_COLUMNS` series subquery `ORDER BY s.name LIMIT 1`, `db/src/books/projection.rs:55-63`), silently discarding the others.

This is latent ambiguity that browse-by-series and series-detail pages will harden around. Fix direction: decide the model. If multi-series is intended, move `series_index` onto `books_series_link` as `(book, series, series_index)`. If single-series is the real model (which the scalar implies), add `UNIQUE(book)` to `books_series_link` to make the 1:1 assumption explicit and stop the read path arbitrarily choosing. Decide before the series pages cement the current arbitrary pick.

---

### Medium Priority — Author photo BLOBs stored inline in DB

`author_photos.bytes` stores the full image (manual upload or OpenLibrary fetch) as a BLOB inside the primary SQLite database (`0008_author_photos.sql:16`), unlike book covers which were deliberately moved out to the filesystem (`covers_dir`; `0003_drop_legacy_tables.sql` dropped the legacy `book_covers` BLOB table). Inline image BLOBs inflate the main DB file, get copied wholesale by VACUUM, sit on the same pages the hot taxonomy/books queries scan (and `list_authors`/`get_author` probe `author_photos` per row via EXISTS, `db/src/browse.rs:76-81`, `db/src/discovery/authors.rs:170-182`), and bloat any backup/replication of the DB.

The project already established the "images on disk, DB holds metadata" pattern for covers; author photos diverge from it for no stated reason. Fix direction: store author photos under a photos dir keyed by author id (mirroring covers per `db/src/pool.rs`), keep only `(source, url, mime, fetched_at)` in the row, and serve from the filesystem. Read/write paths to migrate are at `db/src/author_photos_data.rs:65-73,106-119`.

---

### Medium Priority — Tag cloud recomputes effective CTE per tag

`get_tag_cloud` builds an `effective` CTE (UNION of canonical `books_tags_link` rows + override-extracted subjects via `json_each`), then computes each tag's count as a correlated scalar subquery `(SELECT COUNT(*) FROM effective e WHERE e.tag_id = t.id OR e.tag_name = t.name)` evaluated per surviving tag (up to LIMIT 500). The CTE is not MATERIALIZED, and the per-tag correlated count with an OR across two columns (`tag_id` vs `tag_name`) cannot use an index into the CTE result, so the effective set is effectively recomputed/re-scanned for each tag (`db/src/discovery/tags.rs:49-76`). On a Calibre dump with thousands of subjects this is a full `books_tags_link` scan multiplied by the tag count.

The code comment itself acknowledges "one pass per tag, not one pass total" (`db/src/discovery/tags.rs:39-40`). Fix direction: compute counts in one pass — GROUP BY the effective set's tag key and join to `tags` — rather than a correlated subquery per tag; MATERIALIZE the CTE if the per-tag form is kept.

---

### Low Priority — Redundant indexes duplicate UNIQUE/PK prefixes

Four explicit indexes duplicate an auto-index a UNIQUE/PRIMARY KEY already provides, adding write amplification and storage for no read benefit. (1) `idx_books_uuid` on `books(uuid)` (`0002_normalized_schema.sql:76`) duplicates the implicit unique index from `uuid TEXT NOT NULL UNIQUE` (`0002:25`) — F4.1's "books.uuid needs an index" is already satisfied by the UNIQUE, so this is pure duplication. (2) `idx_book_files_book_id` on `(book_id)` (`0018_multiformat_book_files.sql:39`) is a strict prefix of `idx_book_files_book_format` on `(book_id, format)` (`:40`); migration `0011:11-13`'s own comment admits keeping it "for no meaningful gain." (3) `idx_book_file_parts_lookup` on `(book_file_id, ordinal)` (`0014_audiobook_parts.sql:34`) duplicates the `UNIQUE(book_file_id, ordinal)` auto-index (`:31`). (4) `reading_progress_user_book_idx` on `(user_id, book_id)` (`0013_reading_progress.sql:30-31`) is the leftmost prefix of the `UNIQUE(user_id, book_id, format)` auto-index (`:28`).

Note the sibling indexes `bookmarks_user_book_idx` (`0013:44`) and the two `*_sessions_user_book_idx` (`0013:58,71`) are NOT redundant — those tables have no `(user_id, book_id, ...)` UNIQUE. Fix direction: drop the four redundant indexes in a cleanup migration; EXPLAIN QUERY PLAN confirms lookups fall back to the UNIQUE/composite indexes.

---

### Low Priority — Speculative idx_books_accent_null has no consumer

`idx_books_accent_null` is a partial index `ON books(id) WHERE accent_color IS NULL` (`0006_books_accent.sql:11-12`), justified as supporting "a future 'backfill missing accents' worker job." No such worker exists — the Task enum (`db/src/worker/types.rs:36-96`) has Scan, ScanAudiobooks, GenerateThumbs, ResolveAuthorPhoto, HlsTranscode, RefetchAuthorPhotos, BackfillChapters, but nothing for accents, and a repo grep finds no `accent_color IS NULL` query anywhere. The only accent queries use `accent_color IS NOT NULL` (`db/src/browse.rs:73,186`), which a partial index holding only NULL rows cannot serve.

It's also unlikely to ever help as designed: `accent_color` is written on every New and Changed book during normal reindex (`db/src/sync/books.rs:521,613` via `sanitize_accent_color`), so NULL rows are ones whose extraction failed or that have no cover, and a reindex self-heals them through the normal path. The index thus indexes a small, self-clearing set for a consumer that doesn't exist. Fix direction: drop it; re-add alongside the worker if one is ever built (per the repo's own rule against speculative indexes).

---

### Low Priority — book_files.mtime is a write-only dead column

`book_files` carries both `mtime TEXT` (the OPF `dcterms:modified` Dublin Core value) and `mtime_epoch INTEGER` (the filesystem stat used for incremental reindex). `0009_book_files_fs_metadata.sql:1-15` preserved the TEXT `mtime` "as-is" when adding `mtime_epoch`, and `0018_multiformat_book_files.sql:19,31` still carries it (NOT NULL) on the table recreate. The incremental diff and all change detection use only `mtime_epoch` (`db/src/books/list.rs:104-132,165-175`). A repo grep confirms `book_files.mtime` (TEXT) is never SELECTed back anywhere — it is written on every `book_files` insert (`db/src/sync/books.rs:457-468,624-635`) but never read.

It is dead weight every writer must keep populating (NOT NULL, no default), and it misleads readers into thinking two mtime sources are both live. Fix direction: either drop the column, or if the OPF modified date has display value, rename it (e.g. `opf_modified`) so it isn't mistaken for filesystem state and stop NOT-NULL-requiring it on the write path.

---

### Low Priority — Vestigial app_state table never used

`app_state` `(id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL)` is created and seeded with value 0 in the baseline migration (`0001_initial_schema.sql:7-14`), but no Rust code reads or updates it — a repo-wide grep across `db/`, `server/`, `frontend/`, `shared/`, `mobile/` returns zero references outside the migration. It is a leftover single-row scratch value from the pre-normalization "counter app" placeholder. It's harmless but misleading: the `CHECK(id=1)` singleton pattern signals "holds important global state" when it holds nothing, and a reviewer can't tell what `value` means.

Fix direction: drop it in a cleanup migration. The project already has a typed `settings` KV store (`0001:16`) and a `secrets` table for any genuine server-side scalar need, so there is no reason to keep `app_state`.
