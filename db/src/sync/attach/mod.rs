//! Cross-format auto-attach lookups shared by `sync_books` and
//! `sync_audiobooks`: before INSERTing a new `books` row, decide
//! whether the file actually belongs to an existing book in another
//! format (same work scanned as EPUB + M4B) and record the attachment
//! in `merged_uuids` so the next reindex won't resurrect a duplicate.

use sqlx::Transaction;

/// Reindex protection: has the file at `(library_path, scan_key)` already
/// been merged into / attached to a book? Returns the recorded
/// `(uuid, book_id, format)` when so, so the caller re-attaches against the
/// **stored** ledger uuid rather than a freshly-recomputed path-derived one.
/// Keying on the relative `scan_key` (F2) — not on `stable_uuid` — is what
/// lets an attachment survive a repoint of its scan root. Covers both
/// index-time auto-attach and manual merges, even when the titles no longer
/// match.
pub(super) async fn find_attachment_by_scan_key(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    scan_key: &str,
) -> Result<Option<(String, i64, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64, String)>(
        "SELECT uuid, book_id, format FROM merged_uuids
          WHERE library_path = ? AND scan_key = ?",
    )
    .bind(library_path)
    .bind(scan_key)
    .fetch_optional(&mut **tx)
    .await
}

/// The attachment `uuid` names under `library_path`, as
/// `(book_id, scan_key)`. The move detector's uuids come from both diff
/// projections; this is the lookup for the `merged_uuids`-backed half, and
/// the inverse of [`find_attachment_by_scan_key`] (which keys on the path
/// because it starts from a file on disk).
pub(super) async fn find_attachment_by_uuid(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    uuid: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT book_id, COALESCE(scan_key, '') FROM merged_uuids
          WHERE uuid = ? AND library_path = ?",
    )
    .bind(uuid)
    .bind(library_path)
    .fetch_optional(&mut **tx)
    .await?;
    // A ledger row with no `scan_key` predates migration 0026 and has no
    // path for the caller to move off of.
    Ok(row.filter(|(_, scan_key)| !scan_key.is_empty()))
}

/// Does a **different** ledger row already hold `(library_path,
/// scan_key)`? The ledger's counterpart to the `books`-side
/// `idx_books_scan_key` pre-check, and needed for the same reason even
/// though nothing here can abort a transaction: `merged_uuids` has only the
/// non-unique `idx_merged_uuids_library_scan`, so a blind relocation would
/// silently leave two rows sharing one key, and
/// [`find_attachment_by_scan_key`] reads with `fetch_optional` — a later
/// scan would then resolve that path to an arbitrary one of them.
pub(super) async fn attachment_scan_key_taken(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    scan_key: &str,
    uuid: &str,
) -> Result<bool, sqlx::Error> {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM merged_uuids
          WHERE library_path = ? AND scan_key = ? AND uuid <> ? LIMIT 1",
    )
    .bind(library_path)
    .bind(scan_key)
    .bind(uuid)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(hit.is_some())
}

/// Repoint an attachment's ledger row at `new_scan_key` after its file
/// moved. The uuid is the row's durable handle, so it is deliberately left
/// alone — re-deriving it from the new path would orphan the attachment.
pub(super) async fn relocate_attachment(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
    new_scan_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE merged_uuids SET scan_key = ? WHERE uuid = ?")
        .bind(new_scan_key)
        .bind(uuid)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Conservative title+author match for a brand-new file: the book this
/// file should attach to, if there is **exactly one** candidate with the
/// same normalized title and author that doesn't already have a
/// `format` file. Ambiguity (two candidates) returns `None` — better to
/// surface a duplicate the admin can merge manually than to guess.
/// Global across libraries: the ebook and audiobook roots are separate
/// trees. Returns the candidate's uuid alongside its id so the caller can
/// test it against this scan's Removed bucket (the relocation check).
pub(super) async fn find_attach_target(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    title_norm: &str,
    author_norm: &str,
    format: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    let candidates: Vec<(i64, String)> = sqlx::query_as(
        "SELECT b.id, b.uuid FROM books b
          WHERE b.title_norm = ?1 AND b.author_norm = ?2
            AND NOT EXISTS (SELECT 1 FROM book_files bf
                            WHERE bf.book_id = b.id AND bf.format = ?3)
          ORDER BY b.id LIMIT 2",
    )
    .bind(title_norm)
    .bind(author_norm)
    .bind(format)
    .fetch_all(&mut **tx)
    .await?;
    if candidates.len() == 1 {
        Ok(candidates.into_iter().next())
    } else {
        Ok(None)
    }
}

/// Return whether another file holds the `(book_id, format)` attachment slot.
/// The incoming `scan_key` is excluded so re-attaching the same file proceeds.
pub(super) async fn slot_held_by_other(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    format: &str,
    scan_key: &str,
) -> Result<bool, sqlx::Error> {
    let held: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM merged_uuids
          WHERE book_id = ? AND format = ? AND scan_key <> ? LIMIT 1",
    )
    .bind(book_id)
    .bind(format)
    .bind(scan_key)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(held.is_some())
}

/// Drop the `merged_uuids` row for `(library_path, scan_key)`. Called when a
/// file that previously recorded an attachment is being demoted to its own
/// book (its slot got taken by another file), so its stale ledger entry
/// doesn't keep replaying the attach on every future scan.
pub(super) async fn forget_attachment(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    scan_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM merged_uuids WHERE library_path = ? AND scan_key = ?")
        .bind(library_path)
        .bind(scan_key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Record (or refresh) the `merged_uuids` row for an attached file.
/// `library_path` is the scanned root of the *file*, not the target
/// book's library — the reindex diff filters on it. `scan_key` is the
/// attached file's relative path: the F2 diff key the reindex matches on,
/// so the attachment survives a repoint of the file's scan root.
pub(super) async fn record_attachment(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
    book_id: i64,
    format: &str,
    library_path: &str,
    scan_key: &str,
) -> Result<(), sqlx::Error> {
    // Reuse the uuid of any existing ledger row for the same
    // `(library_path, scan_key)` so a repoint-recomputed `uuid` can't insert a
    // *duplicate* row for the same attached file — the ledger is keyed on the
    // repoint-stable relative path, and `uuid` is just its stored handle.
    let existing: Option<String> =
        sqlx::query_scalar("SELECT uuid FROM merged_uuids WHERE library_path = ? AND scan_key = ?")
            .bind(library_path)
            .bind(scan_key)
            .fetch_optional(&mut **tx)
            .await?;
    let row_uuid = existing.as_deref().unwrap_or(uuid);
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(row_uuid)
    .bind(book_id)
    .bind(format)
    .bind(library_path)
    .bind(scan_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Adopt the new file's cover for the target book when the target has
/// none. Returns the post-commit cover triple (keyed by the **target's**
/// uuid — cover files are uuid-named) when adoption happened.
pub(super) async fn maybe_adopt_cover(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    cover: Option<&(String, Vec<u8>)>,
) -> Result<Option<(String, String, Vec<u8>)>, sqlx::Error> {
    let Some((mime, bytes)) = cover else {
        return Ok(None);
    };
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT uuid, has_cover FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((target_uuid, has_cover)) = row else {
        return Ok(None);
    };
    if has_cover != 0 {
        return Ok(None);
    }
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    Ok(Some((target_uuid, mime.clone(), bytes.clone())))
}

/// Drop the attached `book_files` rows (and `merged_uuids` entries) for
/// removed uuids. The companion to `sync_removed`'s `DELETE FROM books`:
/// removed uuids that were attachments have no `books` row of their own,
/// so the books delete no-ops for them — this cleans up the actual
/// attachment instead. The file delete is scoped per-file on
/// `(book_id, scan_key)` so removing one multi-part `.m4b` sibling leaves
/// the rest of the parts attached. Parts and chapters cascade via the
/// `book_files` FK; the target book survives (possibly fileless, which is
/// a legal state).
pub(super) async fn remove_attached_files(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    removed_uuids: &[String],
) -> Result<(), sqlx::Error> {
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let files_sql = format!(
            "DELETE FROM book_files WHERE id IN
                (SELECT bf.id FROM book_files bf
                   JOIN merged_uuids mu
                     ON mu.book_id = bf.book_id AND mu.scan_key = bf.scan_key
                  WHERE mu.uuid IN ({placeholders}))"
        );
        let mut q = sqlx::query(&files_sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut **tx).await?;

        let mu_sql = format!("DELETE FROM merged_uuids WHERE uuid IN ({placeholders})");
        let mut q = sqlx::query(&mu_sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
