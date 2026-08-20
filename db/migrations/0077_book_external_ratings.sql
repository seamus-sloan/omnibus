-- Community ratings published by the external metadata providers. Cached
-- provider facts, not a metadata override and not the reader's own star
-- rating: several per book, provider-authored, and refreshable.
--
-- Soft-references the durable `books.uuid` (no FK, no CASCADE), matching
-- `metadata_overrides` — a reindex must leave these rows alone.
CREATE TABLE book_external_ratings (
    book_uuid     TEXT    NOT NULL,          -- soft ref: no FK, no CASCADE
    -- The `MetadataProvider` serde tag (`google_books`, `open_library`, …).
    provider      TEXT    NOT NULL,
    -- The score on the provider's own scale, stored raw alongside that scale
    -- rather than normalized on write: every source today is out of 5, so a
    -- future 0–10 one would otherwise need a backfill and a way to tell
    -- already-normalized rows from raw ones.
    rating        REAL    NOT NULL CHECK (rating > 0),
    rating_max    REAL    NOT NULL CHECK (rating_max > 0),
    -- NULL when the provider reports a score but no count — absent, not zero.
    ratings_count INTEGER,
    source_url    TEXT,
    fetched_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    -- Re-applying a candidate updates the source's row in place.
    PRIMARY KEY (book_uuid, provider)
);
