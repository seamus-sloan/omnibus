-- Per-device sync snapshot for native Kobo wireless sync (F4.1, #922).
--
-- The `x-kobo-synctoken` a device echoes back is opaque and never parsed, so it
-- cannot be the cursor. What the protocol actually needs is a record of what
-- each device already holds, which is what lets `library/sync` emit the three
-- entitlement shapes instead of only adds:
--
--   in opted-in set, absent here          -> NewEntitlement
--   in both, books.last_modified is newer -> ChangedProductMetadata + ChangedReadingState
--   present here, no longer opted in      -> ChangedEntitlement{IsRemoved:true}
--
-- `last_modified_seen` stores the `books.last_modified` value that was current
-- when the row was last sent, so the freshness check is a plain comparison and
-- never depends on server clock skew.
--
-- Soft-references `books.uuid` (no FK, no cascade) per the durable-user-data
-- convention: a reindex must not silently empty a device's snapshot and cause a
-- full re-download. A ghosted book instead falls out through the removal arm,
-- which is the honest signal.
CREATE TABLE kobo_device_books (
    device_id          INTEGER NOT NULL REFERENCES kobo_devices(id) ON DELETE CASCADE,
    book_uuid          TEXT    NOT NULL,
    last_modified_seen INTEGER NOT NULL DEFAULT 0,
    synced_at          INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    PRIMARY KEY (device_id, book_uuid)
);

-- The sync diff reads the whole snapshot for one device on every request; the
-- PRIMARY KEY already covers that lookup, so no extra index is needed here.
