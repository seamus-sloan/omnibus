# 1. Where Omnibus stands today

Single SQLite DB, created inline at startup in [frontend/src/db.rs](../../frontend/src/db.rs):

- `books` — one row per file, keyed by `(library_path, filename)`. Dublin Core fields as columns; contributors/identifiers/subjects as **JSON blobs**.
- `book_covers` — BLOBs with FK + ON DELETE CASCADE (manually enforced in `replace_books`).
- `settings`, `library_index_state`, `app_state` (placeholder from the counter demo).

Data flow: [scanner.rs](../../frontend/src/scanner.rs) walks the library path → [ebook.rs](../../frontend/src/ebook.rs) opens each epub with the `epub` crate and pulls DC metadata + cover bytes → [indexer.rs](../../frontend/src/indexer.rs) performs an atomic `replace_books()` every 60 minutes of staleness. The filesystem is **read-only**; the DB is a rebuildable cache.

The gap between this and AudioBookShelf is large — but it's the gap we want to close deliberately, not by cloning ABS's schema mistakes.

---

[← Overview](0-overview.md) · [Next: feature inventory →](2-feature-inventory.md)
