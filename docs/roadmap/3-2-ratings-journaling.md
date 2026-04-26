# F3.2 — Ratings & journaling

**Phase 3 · Personalization** · **Priority:** P2

Per-user star ratings and free-form journal entries per book.

## Objective

1-5 star rating per user per book, plus a free-form markdown journal (multiple dated entries) on each book detail page.

## User / business value

"My library, with my notes" is the pitch for self-hosted over bookstore apps. Rating data also feeds [F3.3 Suggestions](3-3-suggestions.md).

## Technical considerations

- Two tables: `user_ratings(user_id, book_uuid, stars, updated_at)` and `user_journal_entries(id, user_id, book_uuid, body_md, created_at)`.
- Render journal entries with a server-side markdown renderer (`pulldown-cmark`) + sanitization — never trust raw HTML.
- Ratings UI lives in the book detail page's pre-allocated slot from [F1.4](1-4-book-detail.md).

## Book identity & durability

User data (ratings, journals) must survive the following without data loss:

1. Book is cached, user rates it.
2. User edits book metadata (author name, series) — metadata goes into `books.metadata_overrides`, file is unchanged.
3. User changes the library path in settings — pruning removes the old `libraries` row and its `books` rows.
4. On re-index against the new path, the same physical file must resolve to the **same identity** and the rating must still be linked.

**Do not use `book_id` (INT FK with `ON DELETE CASCADE`) for user data.** Cascade-delete is appropriate for ephemeral derived data (covers, FTS index) but not for user-generated data, which cannot be regenerated from the filesystem.

**Use `book_uuid TEXT NOT NULL` with a soft reference instead:**

```sql
CREATE TABLE user_ratings (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid TEXT    NOT NULL,  -- soft ref: no FK, no CASCADE
    stars     INTEGER NOT NULL CHECK (stars BETWEEN 1 AND 5),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, book_uuid)
);
```

When a book is pruned, its rating row becomes *detached* (orphaned) rather than deleted. When the same book reappears under a new path, re-linking is automatic because the UUID matches.

**UUID stability requires a content-based anchor.** The current `stable_uuid(library_path, filename)` scheme breaks this: changing the library root produces a new UUID for every book. Before this feature ships, `stable_uuid` must be replaced with:

1. **Primary:** EPUB `dc:identifier` from the OPF (already parsed during indexing). This is spec-required, survives path changes, library reorganizations from [F0.6](0-6-library-filesystem.md), and metadata edits (which never touch the file).
2. **Fallback:** SHA-256 of the file's byte contents, when `dc:identifier` is absent, empty, or a random per-export UUID (many EPUB editors generate a fresh one each export — detect by checking for the `urn:uuid:` prefix without a stable publisher or ISBN pattern).

A reconciliation step on re-index can attempt to re-link detached user data by `(author, title)` similarity when a UUID still doesn't match — surfaced as "unlinked annotations" in the UI rather than silently lost.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.3 Auth](0-3-auth.md).
- UUID identity fix (replace `stable_uuid(library_path, filename)` with `dc:identifier`-anchored scheme) — **must land before this feature ships**.

---

[← Back to roadmap summary](0-0-summary.md)
