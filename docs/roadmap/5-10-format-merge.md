# F5.10 — Format merge (manual)

**Phase 5 · Admin & hygiene** · **Priority:** P2

Manual merge of duplicate book rows that represent the same logical work in different formats (e.g. an EPUB and an M4B for *Dracula*).

## Objective

When the same book exists as both an ebook and an audiobook (or multiple ebook formats), omnibus currently creates two separate `books` rows because its identity is path-based (`stable_uuid(library_path, filename)`). This initiative lets an admin manually merge those rows into a single book with multiple `book_files` entries, so the landing page shows one card with both format badges and the detail page offers both "Read" and "Listen" CTAs.

## User / business value

- **Single source of truth per work.** Progress, ratings, journal entries, and shelves all attach to one `books.id` instead of being split across format-siblings the user thinks of as one book.
- **Cleaner landing page.** Libraries with both EPUB and audiobook copies no longer show visual duplicates.
- **Prerequisite for future auto-merge.** The manual path ships the underlying primitives (merge transaction, conflict resolution, undo) that a heuristic matcher can later drive without UI.

## Current state

The schema already supports multi-format books:

- `book_files` has `UNIQUE(book_id, format)` — at most one file per format per book.
- The API returns `formats: Vec<String>` per book (JSON aggregation over `book_files`).
- `reading_progress` is keyed on `(user_id, book_id, format)` — separate progress per format on one book works today.
- `format_switcher.rs` renders per-format action rows on the detail page.

The gap is entirely in the **indexer's identity logic** — nothing ever produces a book row with both an EPUB and an audio `book_files` entry, because UUIDs are derived from filenames which always differ across formats.

## Design

### Merge transaction

Given a **source** book (to be absorbed) and a **target** book (to be kept):

1. **Move `book_files`** — re-parent source's `book_files` rows to `target.id`. If a format collision occurs (both have EPUB), reject the merge with an error rather than silently discarding.
2. **`book_file_parts` follow automatically** — they FK to `book_files.id` (not `books.id`), so re-parenting `book_files.book_id` carries the parts along without any additional UPDATE.
3. **Move m2m links** — `books_authors_link`, `books_series_link`, `books_tags_link`: INSERT OR IGNORE from source into target (additive union, no data loss).
4. **Move progress** — `reading_progress` rows: re-parent `book_id` from source to target. The `UNIQUE(user_id, book_id, format)` constraint means format-differentiated progress won't collide (an EPUB progress row and an audio progress row for the same user land cleanly on one book).
5. **Move identifiers** — `book_identifiers`: INSERT OR IGNORE.
6. **Metadata conflict resolution** — target's scanned metadata wins by default. If source carried `metadata_overrides`, shallow-merge them into target's overrides (target keys win on collision).
7. **Cover precedence** — keep target's cover. If target has none and source does, adopt source's.
8. **Tombstone source** — delete the source `books` row (FKs cascade the now-empty shell). Record the merge in an audit row for undo.
9. **UUID stability** — target keeps its UUID. Source's UUID is recorded in the audit row so that if the source file reappears on a future reindex, the indexer can detect "this was merged" and attach it to target rather than recreating a new row.

### Reindex protection

After a merge, the next `reindex_audiobooks` (or `reindex`) will scan the source file, compute its original UUID, find no matching `books` row, and classify it as "New" — recreating the duplicate.

Solution: a `merged_uuids` table (or a column on the audit row) that the diff step consults. When a scanned UUID appears in `merged_uuids`, the indexer attaches the file to the merge target instead of inserting.

### Undo

Record each merge in a `merge_log` table:

| Column | Purpose |
|--------|---------|
| `id` | PK |
| `target_book_id` | The surviving book |
| `source_uuid` | The absorbed book's original UUID |
| `source_metadata` | JSON snapshot of the source row at merge time |
| `merged_at` | Timestamp |
| `undone_at` | NULL until undo; timestamp when reversed |

Undo recreates the source `books` row from the snapshot, moves back the format-specific `book_files` row, and removes the `merged_uuids` entry so future reindexes treat it independently again.

### UI surface

**Option A — detail page action (MVP):**
An admin-only "Merge with…" button on the book detail page opens a search/autocomplete to pick the other book. Preview shows both covers side-by-side with a "Keep left / Keep right" metadata chooser, then confirms.

**Option B — batch from landing page:**
Multi-select mode (checkbox per card), then "Merge selected" in a toolbar. Works well for bulk cleanup after an import. Can ship after Option A proves the transaction logic.

Both surfaces should also be reachable from the [F5.9 library cleanup](5-9-library-cleanup.md) queue — a "Merge" action alongside the existing "Rename" / "Delete" actions when the detector surfaces a suspected format-duplicate.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) (books/book_files split) — already shipped.
- [F0.3 Auth](0-3-auth.md) (admin gate) — already shipped.
- [F5.1 Metadata edit](5-1-metadata-edit.md) (overrides merge logic) — already shipped.

## Future: automatic merge (out of scope for this initiative)

Once the manual merge primitives exist, a heuristic auto-matcher can be layered on top:

1. **Strong signal (auto-apply):** same author (normalized) + title is identical or one is a substring of the other after subtitle stripping.
2. **Weak signal (suggest):** same author + token-set Jaccard ≥ 0.8 on title. Surfaces in the F5.9 cleanup queue for user confirmation.
3. **Identifier match:** if both carry ISBNs that resolve to the same Open Library "work," auto-merge.

This is a separate initiative because the matching heuristics need careful tuning to avoid false positives (e.g. merging "Dune" with "Dune Messiah"), and the manual path provides the escape hatch when auto-merge gets it wrong.

## Risks

- **Reindex race:** if a merge and a reindex run concurrently, the reindex could recreate the source row before the `merged_uuids` guard is committed. Mitigation: the merge transaction and the reindex diff step both run inside `BEGIN IMMEDIATE` — SQLite serializes them.
- **Progress collision on undo:** if a user accumulated new progress on the merged book after the merge, undo would need to decide which book owns that progress. Simplest rule: progress stays on target; undo only splits the file, not the history.

---

[← Back to roadmap summary](0-0-summary.md)
