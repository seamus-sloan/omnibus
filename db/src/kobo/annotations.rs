//! Per-(device, book) annotation sync state for the Kobo Reading Services
//! channel (`kobo_annotations_sync`): first-PATCH adoption, whether the
//! device has actually fetched the book's file, the acked-fingerprint
//! watermark behind `checkforchanges`, and the content fingerprint itself.
//! Annotation rows live device-agnostic in `annotations`.

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::annotations::{served_kobo_annotations_batch, HighlightError, ServedKoboAnnotation};

/// Failure space for the Kobo annotation sync: an underlying highlight
/// operation, or SQL.
#[derive(Debug, thiserror::Error)]
pub enum KoboAnnotationSyncError {
    #[error(transparent)]
    Annotations(#[from] HighlightError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Whether this device has had its first clean PATCH for this book ingested.
/// Until then, `GET .../annotations` must not answer an empty-but-valid set —
/// delete-by-omission would wipe highlights made before wireless sync existed.
pub async fn is_adopted(
    pool: &SqlitePool,
    device_id: i64,
    book_uuid: &str,
) -> Result<bool, KoboAnnotationSyncError> {
    let adopted: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT adopted_at FROM kobo_annotations_sync
         WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device_id)
    .bind(book_uuid)
    .fetch_optional(pool)
    .await?;
    Ok(adopted.flatten().is_some())
}

/// Record that this device has uploaded its annotation state for this book.
/// Idempotent; an existing adoption timestamp is preserved.
pub async fn mark_adopted(
    pool: &SqlitePool,
    device_id: i64,
    book_uuid: &str,
) -> Result<(), KoboAnnotationSyncError> {
    sqlx::query(
        "INSERT INTO kobo_annotations_sync (device_id, book_uuid, adopted_at)
         VALUES (?, ?, strftime('%s','now'))
         ON CONFLICT(device_id, book_uuid) DO UPDATE SET
             adopted_at = COALESCE(kobo_annotations_sync.adopted_at, excluded.adopted_at),
             updated_at = strftime('%s','now')",
    )
    .bind(device_id)
    .bind(book_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that this device has fetched the book's file over the wireless
/// `download` route (#1647). This is the fact [`ack_served`] gates on —
/// serving annotation bytes is not, by itself, proof a device adopted them —
/// and it also makes a downloaded-but-not-yet-adopted book a
/// [`changed_book_uuids`] candidate on its own, without waiting for a PATCH.
/// Idempotent; a repeat download just refreshes the timestamp.
pub async fn mark_downloaded(
    pool: &SqlitePool,
    device_id: i64,
    book_uuid: &str,
) -> Result<(), KoboAnnotationSyncError> {
    sqlx::query(
        "INSERT INTO kobo_annotations_sync (device_id, book_uuid, downloaded_at)
         VALUES (?, ?, strftime('%s','now'))
         ON CONFLICT(device_id, book_uuid) DO UPDATE SET
             downloaded_at = excluded.downloaded_at,
             updated_at    = strftime('%s','now')",
    )
    .bind(device_id)
    .bind(book_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record the fingerprint of the annotation set this device fully downloaded
/// — but only for a book this device is on record as holding
/// ([`mark_downloaded`], #1647). A GET served for a book the device never
/// fetched is a no-op here rather than an ack: without this gate, delivery of
/// the response body was treated as proof of adoption, and a device that
/// never actually saved the file still watermarked past it, stranding the
/// annotations with no redelivery path. Call only once the response body has
/// actually drained — an unacked change keeps the book in `checkforchanges`,
/// which is the protocol's only redelivery mechanism.
pub async fn ack_served(
    pool: &SqlitePool,
    device_id: i64,
    book_uuid: &str,
    fingerprint: &str,
) -> Result<(), KoboAnnotationSyncError> {
    sqlx::query(
        "UPDATE kobo_annotations_sync
            SET acked_fingerprint = ?, updated_at = strftime('%s','now')
          WHERE device_id = ? AND book_uuid = ? AND downloaded_at IS NOT NULL",
    )
    .bind(fingerprint)
    .bind(device_id)
    .bind(book_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

/// Books whose servable annotation set has moved past what this device last
/// acked — the `checkforchanges` answer.
///
/// Candidates are the union of this device's sync-state rows and the user's
/// books holding Kobo-anchored annotations. A pair that is unadopted *and*
/// has nothing to serve is never reported (AC5): reporting it would invite a
/// GET whose empty answer erases the device's pre-sync backlog.
pub async fn changed_book_uuids(
    pool: &SqlitePool,
    user_id: i64,
    device_id: i64,
) -> Result<Vec<String>, KoboAnnotationSyncError> {
    let rows = sqlx::query(
        "SELECT book_uuid, adopted_at, acked_fingerprint
           FROM kobo_annotations_sync WHERE device_id = ?",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?;
    let mut state: std::collections::BTreeMap<String, (bool, Option<String>)> =
        std::collections::BTreeMap::new();
    for row in &rows {
        state.insert(
            row.try_get("book_uuid")?,
            (
                row.try_get::<Option<i64>, _>("adopted_at")?.is_some(),
                row.try_get("acked_fingerprint")?,
            ),
        );
    }

    let annotated: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT book_uuid FROM annotations
         WHERE user_id = ? AND kobo_location IS NOT NULL AND client_id IS NOT NULL",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    // BTreeSet union: dedup + deterministic report order in one shot.
    let candidates: std::collections::BTreeSet<String> =
        annotated.into_iter().chain(state.keys().cloned()).collect();
    let candidate_list: Vec<String> = candidates.iter().cloned().collect();

    // Single batched fetch instead of one query per candidate — this backs
    // the device's polling heartbeat, so it must not scale with the tracked-
    // book count. Mirrors `sync_delta`'s use of `reading_state_for` (delta.rs).
    let served_by_uuid = served_kobo_annotations_batch(pool, user_id, &candidate_list).await?;

    let mut changed = Vec::new();
    for uuid in candidates {
        let served: &[ServedKoboAnnotation] = served_by_uuid
            .get(&uuid)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let (adopted, acked) = state.get(&uuid).cloned().unwrap_or((false, None));
        // Unadopted pairs are servable only once something exists to serve —
        // the same rule the GET handler applies before answering 200.
        if !adopted && served.is_empty() {
            continue;
        }
        let current = fingerprint(served);
        if acked.as_deref() != Some(current.as_str()) {
            changed.push(uuid);
        }
    }
    Ok(changed)
}

/// Content fingerprint of a servable annotation set: SHA-256 over the rows
/// sorted by `client_id`, each field length-prefixed so concatenation is
/// unambiguous. Anchors are excluded on purpose — the device already holds
/// them; membership, color, note, and text are what it needs re-delivered.
pub fn fingerprint(rows: &[ServedKoboAnnotation]) -> String {
    let mut sorted: Vec<&ServedKoboAnnotation> = rows.iter().collect();
    sorted.sort_by(|a, b| a.client_id.cmp(&b.client_id));

    let mut hasher = Sha256::new();
    hasher.update(b"omnibus-kobo-annotations-v1");
    for row in sorted {
        for field in [
            Some(row.client_id.as_str()),
            Some(row.color.as_str()),
            row.note.as_deref(),
            row.text.as_deref(),
        ] {
            match field {
                Some(value) => {
                    hasher.update((value.len() as u64).to_be_bytes());
                    hasher.update(value.as_bytes());
                }
                None => hasher.update(u64::MAX.to_be_bytes()),
            }
        }
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests;
