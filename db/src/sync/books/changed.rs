//! Changed bucket: wipe-and-rewrite per-book link rows, UPDATE the
//! `books` row in place (preserving id + uuid), and re-insert the FTS
//! row. Falls back to a New insert when the diff lost a TOCTOU race or
//! routes to the attach path for cross-format files surfaced under a
//! moved scan_key.

use std::collections::HashMap;

use sqlx::Transaction;

use crate::helpers::scan_key_for;

use super::super::attach;
use super::super::fts::upsert_fts;
use super::shared::{
    attach_ebook_file, insert_book_row, insert_metadata_links, rewrite_book_in_place,
};
use super::{EntityAliasMaps, SyncError};

/// Apply Changed entries: wipe-and-rewrite the per-book link rows for each,
/// UPDATE the `books` row in place (preserving id), and re-insert the FTS
/// row. Returns `(uuid, mime, bytes)` triples for the post-commit cover
/// materialization.
///
/// If the diff said this uuid existed but a concurrent process removed it
/// between Phase A and the write, fall back to inserting it as a new book
/// rather than failing the whole sync over a TOCTOU.
///
/// `alias_maps` is the whole New+Changed batch's reindex-resurrection guard
/// lookup (#1985), pre-resolved by `super::collect_entity_alias_maps`.
pub(super) async fn sync_changed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    changed_books: &[crate::ebook::IndexedBook],
    alias_maps: &EntityAliasMaps,
    mut on_book_written: impl FnMut(&str),
) -> Result<Vec<(String, String, Vec<u8>)>, SyncError> {
    if changed_books.is_empty() {
        return Ok(Vec::new());
    }

    // Pre-compute the `scan_key` (relative path, F2) for each Changed
    // entry so we can batch the id lookup. The diff matches on scan_key,
    // and we carry each row's durable `uuid` back for the cover triple
    // (cover files are uuid-named).
    let all_scan_keys: Vec<String> = changed_books
        .iter()
        .map(|b| scan_key_for(&b.metadata.filename))
        .collect();

    // One batch SELECT per chunk (chunked at 499 to stay under SQLite's
    // 999-parameter cap: 1 bind for library_id + up to 499 scan_key binds).
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

    // Wipe-and-rewrite the per-book link rows for each Changed entry,
    // then UPDATE the `books` row and re-insert FTS. This preserves
    // `books.id` and `books.uuid` — the invariants external callers and
    // F1 soft-refs depend on.
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (b, scan_key) in changed_books.iter().zip(all_scan_keys.iter()) {
        sync_changed_one(
            tx,
            library_id,
            library_path,
            b,
            scan_key,
            &id_map,
            alias_maps,
            &mut changed_covers,
        )
        .await?;
        on_book_written(&b.metadata.filename);
    }
    Ok(changed_covers)
}

/// Apply a single Changed entry — extracted so `sync_changed`'s outer
/// loop stays a clean per-book progress tick.
#[allow(clippy::too_many_arguments)]
async fn sync_changed_one(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    b: &crate::ebook::IndexedBook,
    scan_key: &str,
    id_map: &HashMap<String, (i64, String)>,
    alias_maps: &EntityAliasMaps,
    changed_covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<(), SyncError> {
    let Some((book_id, uuid)) = id_map.get(scan_key).map(|(id, u)| (*id, u.clone())) else {
        // No primary `books` row with this scan_key. Either the file is an
        // attachment on another book (recorded in merged_uuids, matched by
        // `(library_path, scan_key)` so it survives a repoint — refresh that
        // book_files row against the stored ledger uuid, leaving the target
        // book's metadata alone) …
        if let Some((merged_uuid, target_id, format)) =
            attach::find_attachment_by_scan_key(tx, library_path, scan_key).await?
        {
            if attach_ebook_file(
                tx,
                target_id,
                &format,
                library_path,
                &merged_uuid,
                b,
                changed_covers,
            )
            .await?
            {
                return Ok(());
            }
            // Slot taken by a different file: forget the stale ledger row and
            // fall through to promote this file to its own New insert.
            attach::forget_attachment(tx, library_path, scan_key).await?;
        }
        // … or a TOCTOU: the diff said this scan_key existed in the DB,
        // but a concurrent process removed it between Phase A and
        // the write. Promote to a New insert so the file still gets
        // indexed.
        let inserted = insert_book_row(tx, library_id, library_path, b).await?;
        insert_metadata_links(tx, inserted.book_id, &b.metadata, alias_maps).await?;
        // Source the FTS row from the rows we just wrote via the door.
        upsert_fts(tx, inserted.book_id).await?;
        super::super::push_cover(changed_covers, &inserted.uuid, &b.cover);
        return Ok(());
    };

    // Rewrites in place, re-creating the `book_files` row whether the book
    // had one (a real content change) or not (a fileless book whose file
    // returned — the re-attach path, F2). `books.uuid` is preserved.
    rewrite_book_in_place(tx, book_id, &uuid, b, alias_maps, changed_covers).await?;
    Ok(())
}
