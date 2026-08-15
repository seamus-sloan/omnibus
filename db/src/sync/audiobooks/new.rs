//! New bucket: insert canonical `books` + `book_files` + parts + chapters +
//! author-link + FTS rows for each entry. Re-attaches to a same-scan_key
//! fileless row in place to preserve `books.uuid`, and tries the
//! cross-format auto-attach heuristic before minting a fresh row.

use std::collections::{HashMap, HashSet};

use sqlx::Transaction;

use super::super::books::SyncError;
use super::shared::{insert_new_audiobook, rewrite_audiobook_in_place, try_attach_new_audiobook};

/// Apply the New bucket: for each entry try cross-format attach first,
/// otherwise insert a fresh `books` + `book_files` + parts + chapters +
/// author-link + FTS row. Returns the post-commit cover triples.
///
/// `removed_uuids` is this same sync's Removed-bucket output — passed through
/// to the attach heuristic so it can tell a relocation (the matched target's
/// own group just vanished this scan) from a genuine cross-format
/// attachment.
pub(super) async fn sync_audiobooks_new(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    new_books: &[crate::audiobook::IndexedAudiobook],
    removed_uuids: &[String],
    mut on_book_written: impl FnMut(&str),
) -> Result<Vec<(String, String, Vec<u8>)>, SyncError> {
    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    if new_books.is_empty() {
        return Ok(new_covers);
    }
    let removed_this_scan: HashSet<&str> = removed_uuids.iter().map(String::as_str).collect();
    // Pre-fetch every same-scan_key `books` row in one batch (chunked at 499 to
    // stay under SQLite's 999-param cap), keyed on the F2 `scan_key` — mirrors
    // `sync_audiobooks_changed`. New entries carry distinct scan_keys, so each
    // maps at most one existing row and the map never goes stale mid-loop.
    let all_scan_keys: Vec<String> = new_books.iter().map(|b| b.scan_key.clone()).collect();
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

    for b in new_books {
        // Same-scan_key row (a fileless book whose group returned, or a
        // `replace_books` re-add) → rewrite in place, *before* the
        // cross-format attach heuristic, preserving `books.uuid`.
        if let Some((book_id, uuid)) = id_map.get(&b.scan_key).map(|(id, u)| (*id, u.clone())) {
            rewrite_audiobook_in_place(tx, book_id, &uuid, b, &mut new_covers).await?;
            on_book_written(&b.scan_key);
            continue;
        }
        if try_attach_new_audiobook(tx, library_path, b, &removed_this_scan, &mut new_covers)
            .await?
        {
            on_book_written(&b.scan_key);
            continue;
        }
        insert_new_audiobook(tx, library_id, b, &mut new_covers).await?;
        on_book_written(&b.scan_key);
    }
    Ok(new_covers)
}
