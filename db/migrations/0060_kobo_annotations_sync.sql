-- Kobo annotation channel (F4.1, #1278): device identity + per-device sync
-- bookkeeping for the Reading Services routes.
--
-- The device calls `/api/v3/content/...` at the bare origin with no token in
-- the path — the only identity it presents is the `x-kobo-deviceid` header,
-- learned earlier by the tokened /kobo/{token}/v1 path. Resolving that header
-- must never be ambiguous (two rows with one hardware id would leak
-- annotations across users), so: dedup keeping the most-recently-seen row,
-- then enforce uniqueness. The learn path becomes steal-on-learn to match.
UPDATE kobo_devices SET kobo_device_id = NULL
 WHERE kobo_device_id IS NOT NULL
   AND id NOT IN (SELECT kd.id FROM kobo_devices kd
                   WHERE kd.kobo_device_id = kobo_devices.kobo_device_id
                   ORDER BY kd.last_seen_at DESC, kd.id DESC LIMIT 1);
CREATE UNIQUE INDEX kobo_devices_hardware_id_idx
    ON kobo_devices(kobo_device_id) WHERE kobo_device_id IS NOT NULL;

-- Naming consistency with kobo_annotations_sync below: the 0056 library-sync
-- snapshot becomes kobo_books_sync. A rename carries rows, PK, and FK along.
ALTER TABLE kobo_device_books RENAME TO kobo_books_sync;

-- Per-(device, book) annotation sync state. Holds no annotation data — those
-- rows live in `annotations` (0059), device-agnostic. `adopted_at` implements
-- the first-sync backlog guard: until a device's first clean PATCH for a book
-- lands, GET answers 304 so delete-by-omission cannot wipe highlights made
-- before wireless sync was configured. `acked_fingerprint` is the content
-- hash of the last annotation set this device fully downloaded; a moved
-- fingerprint is what checkforchanges reports.
CREATE TABLE kobo_annotations_sync (
    device_id         INTEGER NOT NULL REFERENCES kobo_devices(id) ON DELETE CASCADE,
    book_uuid         TEXT    NOT NULL,
    adopted_at        INTEGER,
    acked_fingerprint TEXT,
    updated_at        INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (device_id, book_uuid)
);
