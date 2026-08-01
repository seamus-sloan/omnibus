//! Web→Kobo annotation materialization: rows holding only an
//! `epub_cfi_range` get a derived `kobo_location` (and a minted
//! `client_id`) so the Reading Services channel can serve them to a
//! device. Gated on the per-user `users.sync_annotations_to_kobo` opt-in;
//! the serving queries never change — a materialized row simply starts
//! matching `kobo_location IS NOT NULL`.

use sqlx::SqlitePool;

use crate::kobo_position::annotation_locations;

/// What one materialization pass accomplished, for logs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DownsyncStats {
    /// Rows that now carry a derived Kobo anchor.
    pub derived: usize,
    /// Candidate rows left without one (point/unparseable CFI, missing
    /// kepub cache or source file, or a derivation miss).
    pub unresolved: usize,
}

/// Book uuids holding this user's web-placeable rows that aren't yet
/// Kobo-placeable — the worklist for [`downsync_book_annotations`].
pub async fn books_needing_kobo_downsync(
    pool: &SqlitePool,
    user_id: i64,
) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT book_uuid FROM annotations
         WHERE user_id = ? AND epub_cfi_range IS NOT NULL AND kobo_location IS NULL",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Materialize `kobo_location` for one (user, book): derive a KoboSpan
/// range for every candidate row in one batch, minting a `client_id` for
/// rows created before one existed so the served id is stable.
/// Deliberately never touches `updated_at`: the wire fingerprint covers
/// membership, so the device re-pulls without a fabricated edit time.
pub async fn downsync_book_annotations(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> anyhow::Result<DownsyncStats> {
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, epub_cfi_range FROM annotations
         WHERE user_id = ? AND book_uuid = ?
           AND epub_cfi_range IS NOT NULL AND kobo_location IS NULL",
    )
    .bind(user_id)
    .bind(book_uuid)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(DownsyncStats::default());
    }
    let mut stats = DownsyncStats {
        derived: 0,
        unresolved: rows.len(),
    };

    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(stats);
    };
    let kepub = crate::kepub_path(book_id);
    if !tokio::fs::try_exists(&kepub).await.unwrap_or(false) {
        return Ok(stats);
    }
    let Some(source) = crate::book_file_path(pool, book_id, "EPUB").await? else {
        return Ok(stats);
    };

    let cfis: Vec<String> = rows.iter().map(|(_, cfi)| cfi.clone()).collect();
    let locations =
        tokio::task::spawn_blocking(move || annotation_locations(&kepub, &source, &cfis)).await??;

    for ((id, snapshot_cfi), location) in rows.iter().zip(locations) {
        let Some(location) = location else { continue };
        // Guarded against a concurrent write: if the row gained an anchor
        // (a device PATCH adopting the same client_id) or its CFI moved
        // since the snapshot, this derivation is stale and must not land.
        let result = sqlx::query(
            "UPDATE annotations
                SET kobo_location = ?, client_id = COALESCE(client_id, ?)
              WHERE id = ? AND kobo_location IS NULL AND epub_cfi_range = ?",
        )
        .bind(&location)
        .bind(uuid::Uuid::new_v4().hyphenated().to_string())
        .bind(id)
        .bind(snapshot_cfi)
        .execute(pool)
        .await?;
        if result.rows_affected() > 0 {
            stats.derived += 1;
            stats.unresolved -= 1;
        }
    }
    Ok(stats)
}

/// Boot-time pass: materialize every opted-in user's pending rows, one
/// book at a time — the retry for rows whose kepub cache didn't exist (or
/// whose text had diverged) when they were written. Cheap once caught up:
/// the worklist query returns nothing for a user with no pending rows.
pub async fn downsync_all_kobo_annotations(pool: &SqlitePool) -> anyhow::Result<DownsyncStats> {
    let users: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE sync_annotations_to_kobo = 1")
            .fetch_all(pool)
            .await?;
    let mut stats = DownsyncStats::default();
    for user_id in users {
        for book_uuid in books_needing_kobo_downsync(pool, user_id).await? {
            match downsync_book_annotations(pool, user_id, &book_uuid).await {
                Ok(s) => {
                    stats.derived += s.derived;
                    stats.unresolved += s.unresolved;
                }
                Err(e) => {
                    tracing::warn!(user_id, book_uuid, error = %e, "annotation downsync failed for book");
                }
            }
        }
    }
    Ok(stats)
}

/// Fire-and-forget materialization after a highlight write: checks the
/// author's opt-in and spawns the per-book pass when it's on. Callers stay
/// on their own latency budget — a highlight create never waits on an
/// EPUB walk.
pub fn spawn_kobo_downsync_if_enabled(pool: SqlitePool, user_id: i64, book_uuid: String) {
    tokio::spawn(async move {
        match crate::auth::sync_annotations_to_kobo(&pool, user_id).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::warn!(user_id, error = %e, "annotation downsync opt-in check failed");
                return;
            }
        }
        if let Err(e) = downsync_book_annotations(&pool, user_id, &book_uuid).await {
            tracing::warn!(user_id, book_uuid, error = %e, "annotation downsync failed");
        }
    });
}

/// Fire-and-forget whole-backlog materialization — the toggle-on hook:
/// every book with pending rows for this user, converted in the
/// background so enabling the setting returns immediately.
pub fn spawn_kobo_downsync_backlog(pool: SqlitePool, user_id: i64) {
    tokio::spawn(async move {
        let books = match books_needing_kobo_downsync(&pool, user_id).await {
            Ok(books) => books,
            Err(e) => {
                tracing::warn!(user_id, error = %e, "annotation downsync worklist failed");
                return;
            }
        };
        for book_uuid in books {
            if let Err(e) = downsync_book_annotations(&pool, user_id, &book_uuid).await {
                tracing::warn!(user_id, book_uuid, error = %e, "annotation downsync failed");
            }
        }
    });
}

#[cfg(test)]
mod tests;
