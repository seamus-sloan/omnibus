-- F2.4b Highlight annotations per user per book.
--
-- Each row stores an EPUB CFI range that identifies the highlighted span
-- within the book's DOM, a color label, and an optional free-text note.
-- The CFI range is the epub.js-native locator so annotations round-trip
-- through rendition.annotations.highlight() without re-parsing.
CREATE TABLE highlights (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id        INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    epub_cfi_range TEXT    NOT NULL,
    color          TEXT    NOT NULL DEFAULT 'amber'
                   CHECK (color IN ('amber', 'green', 'blue', 'rose', 'violet')),
    note           TEXT,
    created_at     INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
CREATE INDEX idx_highlights_user_book ON highlights(user_id, book_id);
