//! Per-device sync delta: diff the caller's opted-in books against what a
//! device already holds (`kobo_books_sync`) and classify each into an add,
//! a change, or a removal.

use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use super::{sync_books, KoboBookRow, KoboError};

/// What `library/sync` should emit for one book on one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncChange {
    /// The device has never seen this book.
    New(KoboBookRow),
    /// The device holds it, but `books.last_modified` has moved since.
    Changed(KoboBookRow),
    /// The book and its metadata are unchanged, but the user's reading state
    /// (status / position) has moved since this device last synced.
    StateChanged(KoboBookRow),
    /// The device holds it and it is no longer opted in — archive it there.
    Removed { book_uuid: String },
}

/// The full delta for one device, in emit order: adds and changes first,
/// removals last.
#[derive(Debug, Default)]
pub struct SyncDelta {
    pub changes: Vec<SyncChange>,
}

impl SyncDelta {
    /// `true` when the device is already up to date.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// How many changes the device still has to apply.
    pub fn len(&self) -> usize {
        self.changes.len()
    }
}

/// Compute what `device_id` still needs, given `user_id`'s opted-in books.
///
/// **Read-only.** The snapshot is advanced by [`record_synced`] once the
/// entitlements have actually been written to the response — a device that
/// drops the connection mid-body must see the same delta again rather than
/// silently losing books. That ordering is the whole reason this is two calls
/// and not one.
pub async fn sync_delta(
    pool: &SqlitePool,
    user_id: i64,
    device_id: i64,
) -> Result<SyncDelta, KoboError> {
    let wanted = sync_books(pool, user_id).await?;
    let held = held_snapshot(pool, device_id).await?;
    let wanted_uuid_list: Vec<String> = wanted.iter().map(|b| b.uuid.clone()).collect();
    let states = super::reading_state_for(pool, user_id, &wanted_uuid_list).await?;

    let mut changes = Vec::new();
    for book in &wanted {
        match held.get(&book.uuid) {
            None => changes.push(SyncChange::New(book.clone())),
            Some(h) if book.last_modified_epoch > h.last_modified_seen => {
                changes.push(SyncChange::Changed(book.clone()));
            }
            // Metadata is current, but the user's status/position may have
            // moved on another surface since this device last synced. Without
            // this arm a book marked Finished on the web is never re-announced
            // to a device that already holds it.
            Some(h)
                if states
                    .get(&book.uuid)
                    .is_some_and(|s| s.state_updated_at > h.synced_at) =>
            {
                changes.push(SyncChange::StateChanged(book.clone()));
            }
            Some(_) => {}
        }
    }

    // Removals last: a device applies the batch in order, and archiving before
    // the adds land would briefly empty a book the same sync is re-adding.
    let wanted_uuids: std::collections::HashSet<&str> =
        wanted.iter().map(|b| b.uuid.as_str()).collect();
    for uuid in held.keys() {
        if !wanted_uuids.contains(uuid.as_str()) {
            changes.push(SyncChange::Removed {
                book_uuid: uuid.clone(),
            });
        }
    }

    Ok(SyncDelta { changes })
}

/// Rows per chunk for the upsert statement: 3 binds/row keeps each statement
/// comfortably under SQLite's 999 bound-parameter cap.
const UPSERT_CHUNK_SIZE: usize = 300;

/// UUIDs per chunk for the `IN (...)` update/delete statements: 1 bind for
/// `device_id` plus one per UUID, well under the cap.
const UUID_CHUNK_SIZE: usize = 900;

/// Advance the device's snapshot to match `changes` — upserting the adds and
/// changes at the `last_modified` they were sent at, and dropping the removals.
///
/// Groups `changes` by operation kind and issues one chunked multi-row
/// statement per group, instead of one round-trip per book — a large opted-in
/// library would otherwise hold SQLite's single writer lock for the whole
/// device sync.
///
/// Call this only after the response body is committed to; see [`sync_delta`].
pub async fn record_synced(
    pool: &SqlitePool,
    device_id: i64,
    changes: &[SyncChange],
) -> Result<(), KoboError> {
    if changes.is_empty() {
        return Ok(());
    }
    let mut upserts: Vec<&KoboBookRow> = Vec::new();
    let mut state_changed: Vec<&str> = Vec::new();
    let mut removed: Vec<&str> = Vec::new();
    for change in changes {
        match change {
            SyncChange::New(book) | SyncChange::Changed(book) => upserts.push(book),
            SyncChange::StateChanged(book) => state_changed.push(book.uuid.as_str()),
            SyncChange::Removed { book_uuid } => removed.push(book_uuid.as_str()),
        }
    }

    let mut tx = pool.begin().await?;
    upsert_synced(&mut tx, device_id, &upserts).await?;
    // State-only: bump the watermark, never `last_modified_seen` — overwriting
    // it here would mark unsent metadata as delivered.
    touch_state_changed(&mut tx, device_id, &state_changed).await?;
    delete_removed(&mut tx, device_id, &removed).await?;
    tx.commit().await?;
    Ok(())
}

/// Upsert one `kobo_books_sync` row per new/changed book, chunked to stay
/// under SQLite's bound-parameter limit.
async fn upsert_synced(
    tx: &mut Transaction<'_, Sqlite>,
    device_id: i64,
    books: &[&KoboBookRow],
) -> Result<(), KoboError> {
    for chunk in books.chunks(UPSERT_CHUNK_SIZE) {
        let rows = std::iter::repeat_n("(?, ?, ?, strftime('%s','now'))", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO kobo_books_sync
                (device_id, book_uuid, last_modified_seen, synced_at)
             VALUES {rows}
             ON CONFLICT(device_id, book_uuid) DO UPDATE SET
                last_modified_seen = excluded.last_modified_seen,
                synced_at          = excluded.synced_at"
        );
        let mut q = sqlx::query(&sql);
        for book in chunk {
            q = q
                .bind(device_id)
                .bind(&book.uuid)
                .bind(book.last_modified_epoch);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Bump `synced_at` for every book whose reading state moved but whose
/// metadata didn't, chunked to stay under SQLite's bound-parameter limit.
async fn touch_state_changed(
    tx: &mut Transaction<'_, Sqlite>,
    device_id: i64,
    uuids: &[&str],
) -> Result<(), KoboError> {
    for chunk in uuids.chunks(UUID_CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE kobo_books_sync SET synced_at = strftime('%s','now')
              WHERE device_id = ? AND book_uuid IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(device_id);
        for uuid in chunk {
            q = q.bind(*uuid);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Drop every `kobo_books_sync` row for a book no longer opted in, chunked to
/// stay under SQLite's bound-parameter limit.
async fn delete_removed(
    tx: &mut Transaction<'_, Sqlite>,
    device_id: i64,
    uuids: &[&str],
) -> Result<(), KoboError> {
    for chunk in uuids.chunks(UUID_CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM kobo_books_sync WHERE device_id = ? AND book_uuid IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(device_id);
        for uuid in chunk {
            q = q.bind(*uuid);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Forget everything a device is holding, so its next sync re-sends the whole
/// opted-in set. Used when a device is re-registered or its token rotated.
pub async fn clear_snapshot(pool: &SqlitePool, device_id: i64) -> Result<(), KoboError> {
    sqlx::query("DELETE FROM kobo_books_sync WHERE device_id = ?")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// What a device already holds for one book.
struct Held {
    last_modified_seen: i64,
    /// When this row was last sent — the watermark a state change is newer
    /// than, since reading state carries no `last_modified` of its own.
    synced_at: i64,
}

/// `book_uuid -> Held` for everything the device holds.
async fn held_snapshot(
    pool: &SqlitePool,
    device_id: i64,
) -> Result<std::collections::BTreeMap<String, Held>, KoboError> {
    let rows = sqlx::query(
        "SELECT book_uuid, last_modified_seen, synced_at
           FROM kobo_books_sync WHERE device_id = ?",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?;
    let mut map = std::collections::BTreeMap::new();
    for row in &rows {
        map.insert(
            row.try_get("book_uuid")?,
            Held {
                last_modified_seen: row.try_get("last_modified_seen")?,
                synced_at: row.try_get("synced_at")?,
            },
        );
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
