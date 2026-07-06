//! New bucket: insert canonical `books` + `book_files` rows for each
//! entry, write its metadata links, and refresh FTS. Re-attaches to a
//! same-scan_key fileless row in place to preserve `books.uuid`, and tries the
//! cross-format auto-attach heuristic before minting a fresh row.

use std::collections::HashMap;

use sqlx::Transaction;

use crate::helpers::scan_key_for;

use super::super::fts::upsert_fts;
use super::shared::{
    insert_book_row, insert_metadata_links, rewrite_book_in_place, try_attach_new_ebook,
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

    // Pre-compute the `scan_key` (relative path, F2) for every New entry so we
    // can batch the "does a `books` row with this scan_key already exist?"
    // lookup — a fileless book whose file returned, or the same file marked
    // New by `replace_books` after being marked missing in the same call. The
    // per-book loop below only consults the in-memory `id_map`; the attach
    // heuristic still runs per-book because it's not a scan_key lookup.
    let all_scan_keys: Vec<String> = new_books
        .iter()
        .map(|b| scan_key_for(&b.metadata.filename))
        .collect();

    // One batch SELECT per chunk (chunked at 499 to stay under SQLite's
    // 999-parameter cap: 1 bind for library_id + up to 499 scan_key binds).
    // Mirrors the same-scan_key pre-fetch in `sync_changed`.
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

    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (b, scan_key) in new_books.iter().zip(all_scan_keys.iter()) {
        // A row with this exact scan_key (relative path) already exists — a
        // fileless book whose file returned, or the same file marked New by
        // `replace_books` after being marked missing in the same call. This is the
        // *same native file*, so rewrite it in place (preserving
        // `books.uuid`) — checked before the cross-format attach heuristic,
        // which would otherwise mis-bind the returning file to the fileless row as
        // an attachment.
        if let Some((book_id, uuid)) = id_map.get(scan_key).map(|(id, u)| (*id, u.clone())) {
            rewrite_book_in_place(tx, book_id, &uuid, b, &mut new_covers).await?;
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
        super::super::push_cover(&mut new_covers, &inserted.uuid, &b.cover);
        on_book_written();
    }
    Ok(new_covers)
}
