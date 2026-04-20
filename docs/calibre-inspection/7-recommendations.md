# 7. Recommendations for Omnibus

Ordered roughly by payoff vs. cost.

1. **Split `books` from `book_files`.** A book is a logical work; files are its formats. The current one-row-per-file schema blocks the epub+m4b+pdf-for-one-book case cleanly. Mirror Calibre's `data` table: `book_files(id, book_id, format, filename, size_bytes, mtime)`.

2. **Normalize authors / series / tags / publishers / languages into tables + m2m link tables.** Ship indices on both sides of every link. This is the prerequisite for Libraries (metadata filters), Search, and efficient browse-by-\* routes.

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

14. **Design the `/api/*` surface as primary.** A single documented REST contract that OPDS and Kobo wrap. Mobile app already hits `/api/*` (see [mobile/src/main.rs](../../mobile/src/main.rs)) — lean into it.

## Cross-reference to roadmap

| Roadmap initiative | Recommendations above |
|---|---|
| [F0.1 Schema refactor](../roadmap/0-1-schema-refactor.md) | 1, 2, 4, 5, 10 |
| [F0.2 Migrations](../roadmap/0-2-migrations.md) | (implicit — avoids runtime ALTER) |
| [F0.3 Auth](../roadmap/0-3-auth.md) | 4, 9 |
| [F0.4 FTS5](../roadmap/0-4-fts5.md) | 3 |
| [F0.5 Background worker](../roadmap/0-5-background-worker.md) | 11 |
| [F0.6 Library filesystem](../roadmap/0-6-library-filesystem.md) | 7, 8 |
| [F1.1 Search](../roadmap/1-1-search.md) | 3, 5 |
| [F1.2 Thumbnails](../roadmap/1-2-thumbnails.md) | 6 |
| [F1.3 Library views](../roadmap/1-3-library-views.md) | 5, 12 |
| [F4.1 Kobo sync](../roadmap/4-1-kobo-sync.md) | 13, 14 |
| [F4.2 OPDS](../roadmap/4-2-opds.md) | 13, 14 |
| [F5.1 Metadata edit](../roadmap/5-1-metadata-edit.md) | 7, 8 |

---

[← API surface](6-api-surface.md) · [Overview](0-overview.md) · [Roadmap summary](../roadmap/0-0-summary.md)
