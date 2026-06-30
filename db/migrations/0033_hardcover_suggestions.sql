-- F3.3 Suggestions ("Readers also enjoyed"). Per-book read-alike results
-- mined from Hardcover community-list co-occurrence, fetched off-request by
-- the F0.5 worker and cached with a 30-day TTL.
--
-- Suggested books are *external* (Hardcover catalog, usually not in the
-- library), so rows store the external metadata directly and soft-reference
-- the source library book by `book_uuid` (no FK, no cascade — durable-identity
-- convention, mirrors `user_ratings` in 0031). A reindex never wipes a cache
-- row; it just goes stale and re-resolves.

-- Cached suggestions: 0..N rows per source book, ordered by `rank`.
CREATE TABLE book_suggestions (
    book_uuid      TEXT    NOT NULL,              -- source library book (soft ref)
    rank           INTEGER NOT NULL,              -- 0-based, by co-listing count desc
    hardcover_id   INTEGER NOT NULL,              -- external Hardcover book id
    hardcover_slug TEXT,                           -- outbound /books/{slug} URL (may be null)
    title          TEXT    NOT NULL,
    author         TEXT    NOT NULL,
    list_count     INTEGER NOT NULL DEFAULT 0,     -- co-listing count (relevance signal)
    cover_mime     TEXT,                           -- null when no cover was fetched
    cover_bytes    BLOB,
    PRIMARY KEY (book_uuid, rank)
);

-- Per-book cache marker carrying the 30-day TTL clock and the de-duplication
-- state. `pending` = a resolution was enqueued and is in flight (debounces a
-- burst of viewers into one Hardcover round-trip); `resolved` = results cached
-- (`book_suggestions` may hold 0..N rows); `empty` = sticky negative marker
-- (Hardcover matched nothing usable — stays quiet until the TTL lapses).
CREATE TABLE book_suggestion_state (
    book_uuid  TEXT PRIMARY KEY,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'resolved', 'empty')),
    fetched_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
