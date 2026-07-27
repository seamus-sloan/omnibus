-- Kobo annotation channel (F4.1, #1278): one `annotations` table for every
-- surface's highlights and notes.
--
-- A Kobo anchors an annotation with a KoboSpan range (`span#kobo.N.M` +
-- char offsets inside kepubify-injected wrappers), not an EPUB CFI, so
-- `epub_cfi_range NOT NULL` locks the device out of the store entirely. Same
-- shape as 0058's fix for `reading_progress`: relax the anchor to "one of
-- some kind" — a CFI, a raw Kobo location, or both — and keep the Kobo half
-- opaque (echoed back to the device verbatim, parsed by nobody). Rows with
-- both anchors become possible once a CFI↔KoboSpan converter exists.
--
-- The rename (highlights → annotations) reflects the widened contents: web
-- highlights/notes and Kobo annotations share these rows, and nothing on a
-- row references a device. `client_id` stays the single idempotency
-- namespace — a Kobo's device-minted annotation uuid is just a client-minted
-- id. `updated_at` is new: edits bump it, and the Kobo channel serves it as
-- the annotation's last-modified time.
--
-- SQLite cannot drop NOT NULL in place, so: full rebuild per 0027's pattern.

CREATE TABLE annotations (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid      TEXT    NOT NULL,
    epub_cfi_range TEXT,
    -- The device's annotation `location` object, stored verbatim as JSON.
    -- Deliberately opaque, like reading_progress.kobo_location (0058).
    kobo_location  TEXT,
    color          TEXT    NOT NULL DEFAULT 'amber'
                   CHECK (color IN ('amber','green','blue','rose','violet')),
    note           TEXT,
    created_at     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    text           TEXT,
    client_id      TEXT,
    updated_at     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    -- An annotation is always placeable on at least one surface.
    CHECK (epub_cfi_range IS NOT NULL OR kobo_location IS NOT NULL)
);
INSERT INTO annotations
    (id, user_id, book_uuid, epub_cfi_range, color, note, created_at, text,
     client_id, updated_at)
SELECT id, user_id, book_uuid, epub_cfi_range, color, note, created_at, text,
       client_id, created_at
  FROM highlights;
DROP TABLE highlights;

-- The four highlight indexes, recreated under the new name. The covering
-- index is load-bearing: an EXPLAIN QUERY PLAN test pins the list read to it.
CREATE INDEX idx_annotations_user_book ON annotations(user_id, book_uuid);
CREATE INDEX idx_annotations_book_uuid ON annotations(book_uuid);
CREATE INDEX idx_annotations_user_book_created
    ON annotations(user_id, book_uuid, created_at ASC);
CREATE UNIQUE INDEX annotations_user_client_id_idx
    ON annotations(user_id, client_id) WHERE client_id IS NOT NULL;
