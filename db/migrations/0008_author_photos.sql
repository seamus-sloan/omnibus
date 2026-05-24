-- F1.11 Author profile pictures.
--
-- Cache for resolved author profile photos. One row per author; rows are
-- created/replaced by the `Task::ResolveAuthorPhoto` worker or by admin
-- upload through `PUT /api/authors/:id/photo`.
--
-- `source = 'letter'` is a negative-cache marker — `bytes` and `mime` are
-- NULL and the GET handler 404s, leaving the typographic letter avatar in
-- place. Admin DELETE removes the row so the next view re-queues
-- resolution (i.e. cache invalidation is admin-driven only — see the
-- F1.11 roadmap for the rationale).
CREATE TABLE author_photos (
    author_id   INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN ('manual', 'openlibrary', 'letter')),
    url         TEXT,
    bytes       BLOB,
    mime        TEXT,
    fetched_at  TEXT NOT NULL DEFAULT (datetime('now')),
    -- Invariant: a 'letter' marker is a negative-cache hit and carries no
    -- image bytes; non-letter sources must carry both bytes and mime. Enforced
    -- at the schema level so `get_author_photo` / `AuthorDetail.has_photo` can
    -- trust the row shape without special-casing malformed rows.
    CHECK (
        (source =  'letter' AND bytes IS NULL     AND mime IS NULL)
     OR (source <> 'letter' AND bytes IS NOT NULL AND mime IS NOT NULL)
    )
);
