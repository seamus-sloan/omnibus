//! Per-(device, book) annotation sync state for the Kobo Reading Services
//! channel (`kobo_annotations_sync`, #1278): first-PATCH adoption, the
//! acked-fingerprint watermark behind `checkforchanges`, and the content
//! fingerprint itself. Annotation rows live device-agnostic in `annotations`.

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::annotations::{served_kobo_annotations, HighlightError, ServedKoboAnnotation};

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

/// Record the fingerprint of the annotation set this device fully downloaded.
/// Call only once the response body has actually drained — an unacked change
/// keeps the book in `checkforchanges`, which is the protocol's only
/// redelivery mechanism.
pub async fn ack_served(
    pool: &SqlitePool,
    device_id: i64,
    book_uuid: &str,
    fingerprint: &str,
) -> Result<(), KoboAnnotationSyncError> {
    sqlx::query(
        "INSERT INTO kobo_annotations_sync (device_id, book_uuid, acked_fingerprint)
         VALUES (?, ?, ?)
         ON CONFLICT(device_id, book_uuid) DO UPDATE SET
             acked_fingerprint = excluded.acked_fingerprint,
             updated_at        = strftime('%s','now')",
    )
    .bind(device_id)
    .bind(book_uuid)
    .bind(fingerprint)
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

    let mut candidates: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT book_uuid FROM annotations
         WHERE user_id = ? AND kobo_location IS NOT NULL AND client_id IS NOT NULL",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    for uuid in state.keys() {
        if !candidates.contains(uuid) {
            candidates.push(uuid.clone());
        }
    }
    candidates.sort();

    let mut changed = Vec::new();
    for uuid in candidates {
        let served = served_kobo_annotations(pool, user_id, &uuid).await?;
        let (adopted, acked) = state.get(&uuid).cloned().unwrap_or((false, None));
        // Unadopted pairs are servable only once something exists to serve —
        // the same rule the GET handler applies before answering 200.
        if !adopted && served.is_empty() {
            continue;
        }
        let current = fingerprint(&served);
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
