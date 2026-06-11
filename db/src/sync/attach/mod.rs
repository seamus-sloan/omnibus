//! Cross-format auto-attach lookups shared by `sync_books` and
//! `sync_audiobooks`. When the indexer is about to INSERT a brand-new
//! `books` row, these decide whether the file instead belongs to an
//! existing book in another format (the same work scanned as both an
//! EPUB and an M4B), and record the attachment in `merged_uuids` so the
//! next reindex classifies the file against the attached `book_files`
//! row instead of resurrecting a duplicate.

use sqlx::Transaction;

/// Reindex protection: has this uuid already been merged into / attached
/// to a book? Returns the target `(book_id, format)` when so. Covers
/// both index-time auto-attach and manual merges, and works even when
/// the titles no longer match.
pub(super) async fn attach_target_by_uuid(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, String)>("SELECT book_id, format FROM merged_uuids WHERE uuid = ?")
        .bind(uuid)
        .fetch_optional(&mut **tx)
        .await
}

/// Conservative title+author match for a brand-new file: the book this
/// file should attach to, if there is **exactly one** candidate with the
/// same normalized title and author that doesn't already have a
/// `format` file. Ambiguity (two candidates) returns `None` — better to
/// surface a duplicate the admin can merge manually than to guess.
/// Global across libraries: the ebook and audiobook roots are separate
/// trees.
pub(super) async fn find_attach_target(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    title_norm: &str,
    author_norm: &str,
    format: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let candidates: Vec<i64> = sqlx::query_scalar(
        "SELECT b.id FROM books b
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
    Ok(match candidates.as_slice() {
        [only] => Some(*only),
        _ => None,
    })
}

/// Record (or refresh) the `merged_uuids` row for an attached file.
/// `library_path` is the scanned root of the *file*, not the target
/// book's library — the reindex diff filters on it.
pub(super) async fn record_attachment(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
    book_id: i64,
    format: &str,
    library_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, ?, ?)",
    )
    .bind(uuid)
    .bind(book_id)
    .bind(format)
    .bind(library_path)
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
/// attachment instead. Parts and chapters cascade via the `book_files`
/// FK; the target book survives (possibly fileless, which is a legal
/// state).
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
                     ON mu.book_id = bf.book_id AND mu.format = bf.format
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
