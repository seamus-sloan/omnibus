# F0.1 — Schema refactor

**Phase 0 · Foundations** · **Priority:** P0

Split `books` (logical work) from `book_files` (one row per format). Normalize `authors`, `series`, `tags`, `publishers`, `languages` into tables with m2m link tables.

## Objective

Replace the current denormalized single-table schema (one row per file, JSON-blob relations) with a normalized schema that cleanly models a book as a logical work with one or more physical files, and supports efficient filter/browse on contributors, series, tags, publishers, and languages.

## User / business value

Unblocks:

- **Libraries** ([F3.1](3-1-libraries.md)) — metadata filter rules need real columns.
- **Search** ([F1.1](1-1-search.md)) — FTS5 needs structured content to index.
- **Browse-by-author/series/tag** — impossible to do efficiently from JSON blobs.
- **Multi-format delivery** — read the epub on web, listen to m4b in the car, send epub to Kindle — all the same work.

## Technical considerations

- Mirror Calibre's `data` table layout for path compatibility — see [schema details](../calibre-inspection/5-schema-details.md).
- `book_files(id, book_id, format, filename, size_bytes, mtime)` — extension is a column, not a filename-join.
- Add indices on every m2m reverse column and on `books.uuid`, `books.last_modified`, `books.sort`, `books.series_index`.
- `COLLATE NOCASE` on every searchable string column.
- Keep the filesystem-as-truth, DB-as-cache invariant — nothing here makes the DB authoritative.

## Dependencies

- [F0.2 Migrations](0-2-migrations.md) must land first or concurrently.

## Risks

- Touches every read and write path currently in the repo.
- Needs a re-index after deploy — acceptable since the DB is already a rebuildable cache.

## Open questions

Blocked by open questions 1 (cover storage) and 2 (scan paths) in the [summary](0-0-summary.md#open-questions).

---

[← Back to roadmap summary](0-0-summary.md)
