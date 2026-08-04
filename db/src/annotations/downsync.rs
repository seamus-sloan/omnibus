//! Web→Kobo annotation materialization: rows holding only an
//! `epub_cfi_range` get a derived `kobo_location` (and a minted
//! `client_id`) so the Reading Services channel can serve them to a
//! device. The serving queries never change — a materialized row simply
//! starts matching `kobo_location IS NOT NULL`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use sqlx::SqlitePool;

use crate::kobo_position::annotation_locations;
use crate::worker::{Task, Worker};

/// Process-wide guard for [`spawn_kobo_downsync`]: the key of every
/// `(user_id, book_uuid)` pair with a materialization task currently
/// in flight.
fn inflight_downsyncs() -> &'static Mutex<HashSet<(i64, String)>> {
    static INFLIGHT: OnceLock<Mutex<HashSet<(i64, String)>>> = OnceLock::new();
    INFLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Removes its key from [`inflight_downsyncs`] on drop, not just on the
/// task's normal-completion path — so a panic inside the task, or the task
/// being aborted, still releases the key instead of permanently suppressing
/// future downsyncs for it.
struct InflightGuard {
    key: (i64, String),
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        inflight_downsyncs()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// What one materialization pass accomplished, for logs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DownsyncStats {
    /// Rows that now carry a derived Kobo anchor.
    pub derived: usize,
    /// Candidate rows left without one (point/unparseable CFI, missing
    /// kepub cache or source file, or a derivation miss).
    pub unresolved: usize,
}

/// Materialize `kobo_location` for one (user, book): derive a KoboSpan
/// range for every candidate row in one batch, minting a `client_id` for
/// rows created before one existed so the served id is stable.
/// Deliberately never touches `updated_at`: the wire fingerprint covers
/// membership, so the device re-pulls without a fabricated edit time.
///
/// `worker` is what makes a cold KEPUB cache recoverable: a KoboSpan anchor
/// can only be derived against the converted file, so without it every
/// candidate row stays unresolved *forever* — nothing else on this path
/// rebuilds the cache, and an unmaterialized row never moves the wire
/// fingerprint, so `checkforchanges` never invites the GET that would retry.
/// Pass `None` only when a conversion must not be requested: from the
/// worker's own post-conversion hook, or from a test.
pub async fn downsync_book_annotations(
    pool: &SqlitePool,
    worker: Option<&Arc<Worker>>,
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
        // Idempotent and serialized per book by the worker; do NOT await —
        // callers are a write path or a boot pass, and the conversion's own
        // completion re-runs this via `downsync_book_id_annotations`.
        // Mirrors `derive_span_for_cfi` on the reading-state path.
        if let Some(worker) = worker {
            worker.post(Task::KepubConvert { book_id });
        }
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

/// Boot-time pass: materialize every pending (user, book) pair — the retry
/// for rows whose kepub cache didn't exist (or whose text had diverged)
/// when they were written, and the backlog conversion for highlights made
/// before down-sync existed. Cheap once caught up: the worklist query
/// returns nothing.
///
/// Passing `worker` here is what clears a backlog stranded by a cold cache:
/// the worklist is bounded by "books the user highlighted on the web that
/// haven't reached a device", so the conversions it requests are few, deduped
/// per book by the worker's `kepub:{book_id}` resource key, and skipped
/// outright for any book whose cache is already warm.
pub async fn downsync_all_kobo_annotations(
    pool: &SqlitePool,
    worker: Option<&Arc<Worker>>,
) -> anyhow::Result<DownsyncStats> {
    let pairs: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT user_id, book_uuid FROM annotations
         WHERE epub_cfi_range IS NOT NULL AND kobo_location IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut stats = DownsyncStats::default();
    for (user_id, book_uuid) in pairs {
        match downsync_book_annotations(pool, worker, user_id, &book_uuid).await {
            Ok(s) => {
                stats.derived += s.derived;
                stats.unresolved += s.unresolved;
            }
            Err(e) => {
                tracing::warn!(user_id, book_uuid, error = %e, "annotation downsync failed for book");
            }
        }
    }
    Ok(stats)
}

/// Materialize every user's pending rows for one book id — the hook the
/// worker runs after a successful KEPUB conversion, so the conversion a cold
/// cache asked for actually lands the highlights it was asked for instead of
/// waiting on the next write or the next boot. Also covers a cache warmed for
/// an unrelated reason (a device download, a Send-to-Kobo export).
///
/// Requests no conversion of its own: the cache is fresh by construction here,
/// and a failed derivation must not re-enter the task that called it.
pub async fn downsync_book_id_annotations(
    pool: &SqlitePool,
    book_id: i64,
) -> anyhow::Result<DownsyncStats> {
    let Some(book_uuid): Option<String> = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(DownsyncStats::default());
    };
    let user_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM annotations
         WHERE book_uuid = ? AND epub_cfi_range IS NOT NULL AND kobo_location IS NULL",
    )
    .bind(&book_uuid)
    .fetch_all(pool)
    .await?;

    let mut stats = DownsyncStats::default();
    for user_id in user_ids {
        let s = downsync_book_annotations(pool, None, user_id, &book_uuid).await?;
        stats.derived += s.derived;
        stats.unresolved += s.unresolved;
    }
    Ok(stats)
}

/// Fire-and-forget materialization after a highlight write. Callers stay
/// on their own latency budget — a highlight create never waits on an
/// EPUB walk.
///
/// Coalesced per `(user_id, book_uuid)` via [`inflight_downsyncs`]: several
/// highlights written on the same book in quick succession would otherwise
/// each fan out their own full-book recompute, since every call re-walks
/// every still-unresolved row on the book rather than just its own row. A
/// call that finds one already running for the same key skips the spawn —
/// this is a best-effort guard, not a correctness one: a row written after
/// the in-flight task has already read its worklist stays unresolved until
/// the *next* highlight write or the boot-time [`downsync_all_kobo_annotations`]
/// pass, rather than being covered by the task it skipped spawning behind.
pub fn spawn_kobo_downsync(
    pool: SqlitePool,
    worker: Option<Arc<Worker>>,
    user_id: i64,
    book_uuid: String,
) {
    let key = (user_id, book_uuid.clone());
    {
        let mut inflight = inflight_downsyncs()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inflight.insert(key.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let _guard = InflightGuard { key };
        if let Err(e) = downsync_book_annotations(&pool, worker.as_ref(), user_id, &book_uuid).await
        {
            tracing::warn!(user_id, book_uuid, error = %e, "annotation downsync failed");
        }
    });
}

#[cfg(test)]
mod tests;
