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

When a book is pruned, its rating row becomes *detached* (orphaned) rather than deleted. A library-root *repoint* detaches nothing — F2 preserves `books.uuid` (see below) — so detach only happens when a file is genuinely removed; if that file later returns at the same relative path, re-linking is automatic because the uuid is preserved.

**UUID stability has landed (F2).** This durability depends on a stable `books.uuid`. F2 has since shipped: `books.uuid` is a durable stored value (minted once, never recomputed) and the reindex diff matches disk-vs-DB on a relative-path `scan_key`, so changing the library root preserves every uuid. (This superseded an earlier proposal to anchor the uuid on EPUB `dc:identifier` / content-hash, which was dropped — see [F2](../design/db-review-f2-stable-uuid-identity.md).)

A reconciliation step on re-index can still attempt to re-link detached user data by `(author, title)` similarity when no uuid matches — the case where a library was *removed* and re-added, minting fresh uuids — surfaced as "unlinked annotations" in the UI rather than silently lost.

**Reconcile/GC shape inherited from F10.** The orphan reconcile + GC mechanism is built first for `metadata_overrides` under [F10](../design/db-review-f10-override-gc.md), and the user-data tables here reuse it. Two constraints carry over:

1. **User data must soft-detach, never hard-delete.** F10's orphan reconcile/GC soft-detaches in *both* arms (post-reindex *and* explicit library removal) — only a *user-driven* override deletion hard-deletes. An override is regenerable (it can be re-entered) so its eventual purge is tolerable; a journal entry is **not** regenerable, so `user_ratings` / `user_journal_entries` must use the soft-detach disposition (a `detached_at` marker, read-path filtered, retained for a grace window, hard-purged only after a long retention) and never be the target of a hard-delete GC.
2. **The admin "unlinked edits/annotations" UI lands here.** F10 builds the detach + GC data layer now but defers the admin surface to this feature, so `metadata_overrides` and user-data orphans are presented through one consistent "unlinked" view when F3.2 ships.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.3 Auth](0-3-auth.md).
- UUID identity (F2: durable stored `books.uuid` + relative-path `scan_key`) — **landed**; this feature depends on it.

---

[← Back to roadmap summary](0-0-summary.md)
