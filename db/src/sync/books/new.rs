//! New bucket: insert canonical `books` + `book_files` rows for each
//! entry, write its metadata links, and refresh FTS. Re-attaches to a
//! same-scan_key fileless row in place to preserve `books.uuid`, and tries the
//! cross-format auto-attach heuristic before minting a fresh row.

use sqlx::Transaction;

use crate::helpers::scan_key_for;

use super::super::fts::upsert_fts;
use super::shared::{
    existing_by_scan_keys, insert_book_row, insert_metadata_links, rewrite_book_in_place,
    try_attach_new_ebook,
};

/// Insert a batch of New entries: canonical `books` + `book_files` row,
/// metadata link rows, FTS row. Returns the post-commit cover triples.
pub(super) async fn sync_new(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    new_books: &[crate::ebook::IndexedBook],
    mut on_book_written: impl FnMut(),
) -> Result<Vec<(String, String, Vec<u8>)>, sqlx::Error> {
    if new_books.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-resolve the "does a row with this exact scan_key already exist?"
    // check in one query per chunk before the loop, mirroring the Changed
    // bucket's `id_map`. The per-book loop then looks up in memory — no SELECT
    // per new book — and only the cross-format attach heuristic (which needs
    // the target's own state) stays inside the loop.
    let all_scan_keys: Vec<String> = new_books
        .iter()
        .map(|b| scan_key_for(&b.metadata.filename))
        .collect();
    let existing = existing_by_scan_keys(tx, library_id, &all_scan_keys).await?;

    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (b, scan_key) in new_books.iter().zip(all_scan_keys.iter()) {
        // A row with this exact scan_key (relative path) already exists — a
        // fileless book whose file returned, or the same file marked New by
        // `replace_books` after being marked missing in the same call. This is the
        // *same native file*, so rewrite it in place (preserving
        // `books.uuid`) — checked before the cross-format attach heuristic,
        // which would otherwise mis-bind the returning file to the fileless row as
        // an attachment.
        if let Some((book_id, uuid)) = existing.get(scan_key) {
            rewrite_book_in_place(tx, *book_id, uuid, b, &mut new_covers).await?;
            on_book_written();
            continue;
        }
        if try_attach_new_ebook(tx, library_path, b, &mut new_covers).await? {
            on_book_written();
            continue;
        }
        let inserted = insert_book_row(tx, library_id, library_path, b).await?;
        insert_metadata_links(tx, inserted.book_id, &b.metadata).await?;
        upsert_fts(tx, inserted.book_id).await?;
        if let Some((mime, bytes)) = &b.cover {
            new_covers.push((inserted.uuid, mime.clone(), bytes.clone()));
        }
        on_book_written();
    }
    Ok(new_covers)
}
