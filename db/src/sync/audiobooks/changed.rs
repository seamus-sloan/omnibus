//! Changed bucket: batch-resolve uuid → book_id, then per book either
//! refresh the attached file row (uuid lives in `merged_uuids`), promote to
//! a New insert (TOCTOU — diff said the uuid existed but a concurrent
//! process removed it), or wipe-and-rewrite the file/parts/author rows in
//! place.

use std::collections::HashMap;

use sqlx::Transaction;

use super::super::attach;
use super::super::books::SyncError;
use super::shared::{attach_audiobook_file, insert_new_audiobook, rewrite_audiobook_in_place};

/// Apply the Changed bucket: batch-resolve uuid → book_id, then per book
/// either refresh the attached file row (uuid lives in `merged_uuids`),
/// promote to a New insert (TOCTOU — diff said the uuid existed but a
/// concurrent process removed it), or wipe-and-rewrite the file/parts/
/// author rows in place. Returns `(uuid, mime, bytes)` triples for the
/// post-commit cover materialization.
pub(super) async fn sync_audiobooks_changed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    changed_books: &[crate::audiobook::IndexedAudiobook],
    mut on_book_written: impl FnMut(),
) -> Result<Vec<(String, String, Vec<u8>)>, SyncError> {
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    if changed_books.is_empty() {
        return Ok(changed_covers);
    }
    // Pre-fetch all book ids in one batch query (chunked at 499 to stay
    // under SQLite's 999-parameter cap), keyed on the F2 `scan_key` (the
    // group's relative path); carry each row's durable `uuid` back for the
    // cover triple.
    let all_scan_keys: Vec<String> = changed_books.iter().map(|b| b.scan_key.clone()).collect();
    let mut id_map: HashMap<String, (i64, String)> = HashMap::new();
    for chunk in all_scan_keys.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let id_sql = format!(
            "SELECT scan_key, id, uuid FROM books
              WHERE library_id = ? AND scan_key IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, i64, String)>(&id_sql).bind(library_id);
        for sk in chunk {
            q = q.bind(sk);
        }
        for (sk, id, uuid) in q.fetch_all(&mut **tx).await? {
            id_map.insert(sk, (id, uuid));
        }
    }

    for b in changed_books {
        let Some((book_id, uuid)) = id_map.get(&b.scan_key).map(|(id, u)| (*id, u.clone())) else {
            // No primary books row with this scan_key — either an attachment
            // on another book (matched by the repoint-stable
            // `(library_path, scan_key)`) or a TOCTOU promote to a New insert.
            // (A fileless book whose group returned still has its books row,
            // so it takes the update branch below and re-creates book_files.)
            if let Some((_uuid, target_id, format)) =
                attach::find_attachment_by_scan_key(tx, library_path, &b.scan_key).await?
            {
                if attach_audiobook_file(
                    tx,
                    target_id,
                    &format,
                    library_path,
                    b,
                    &mut changed_covers,
                )
                .await?
                {
                    on_book_written();
                    continue;
                }
                // Slot taken by a different file: forget the stale ledger row
                // and fall through to insert this file as its own book.
                attach::forget_attachment(tx, library_path, &b.scan_key).await?;
            }
            insert_new_audiobook(tx, library_id, b, &mut changed_covers).await?;
            on_book_written();
            continue;
        };

        rewrite_audiobook_in_place(tx, book_id, &uuid, b, &mut changed_covers).await?;
        on_book_written();
    }
    Ok(changed_covers)
}
