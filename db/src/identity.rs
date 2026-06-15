//! Boot backfill for the F2 `scan_key` diff key.
//!
//! `books.uuid` became a durable, minted-once identity and a new
//! `books.scan_key` (the library-relative path) took over the Phase-A diff
//! (migration 0026). Rows indexed before that migration have `scan_key IS
//! NULL`; this fills them in **purely from stored columns** — no filesystem
//! reads — so it is safe against in-memory test DBs and idempotent (a no-op
//! once caught up). Mirrors [`crate::normalize::backfill_norm_columns`] and
//! runs from `init_db` right after it.

use sqlx::SqlitePool;

/// Reconstruct a book/attachment's `scan_key` (its scanner-relative path)
/// from the stored `(path, filename, format)` and the part count, matching
/// exactly what the Phase-A walk would emit so the diff doesn't
/// misclassify a backfilled row as New.
///
/// A folder of `.mp3` chapters has a directory `scan_key` with **no**
/// extension (its `book_file_parts` count is > 1); every other shape — an
/// `.epub`, a single `.m4b`/`.m4a`, or a single `.mp3` — is a file whose
/// `scan_key` carries the lowercased extension.
fn reconstruct_scan_key(path: &str, filename: &str, format: &str, part_count: i64) -> String {
    let is_mp3_folder = format.eq_ignore_ascii_case("MP3") && part_count > 1;
    let leaf = if is_mp3_folder {
        filename.to_string()
    } else {
        format!("{filename}.{}", format.to_ascii_lowercase())
    };
    if path.is_empty() {
        leaf
    } else {
        format!("{path}/{leaf}")
    }
}

/// One-time backfill of `books.scan_key` + `merged_uuids.scan_key` for rows
/// indexed before migration 0026. Idempotent — only touches rows where
/// `scan_key IS NULL` — and pure DB work, so it runs on every boot from
/// `init_db` and against in-memory test DBs.
pub async fn backfill_scan_keys(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    backfill_books_scan_keys(pool).await?;
    backfill_merged_scan_keys(pool).await?;
    Ok(())
}

/// Fill `books.scan_key` from each book's **native** `book_files` row — the
/// format that is *not* recorded in `merged_uuids` (attachments keep their
/// own key). A fileless ghost has no `book_files` row, but ghosts are a
/// post-0026 concept and always carry a `scan_key`, so the INNER JOIN never
/// drops a row that needs filling.
async fn backfill_books_scan_keys(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let rows: Vec<(i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT b.id, b.path, bf.filename, bf.format,
                (SELECT COUNT(*) FROM book_file_parts p WHERE p.book_file_id = bf.id)
           FROM books b
           JOIN book_files bf ON bf.book_id = b.id
          WHERE b.scan_key IS NULL
            AND NOT EXISTS (SELECT 1 FROM merged_uuids mu
                             WHERE mu.book_id = b.id AND mu.format = bf.format)",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut seen = std::collections::HashSet::new();
    let mut tx = pool.begin().await?;
    for (id, path, filename, format, part_count) in rows {
        // A book with two native formats would appear twice; the first
        // reconstruction wins (any native file locates the same book).
        if !seen.insert(id) {
            continue;
        }
        let scan_key = reconstruct_scan_key(&path, &filename, &format, part_count);
        sqlx::query("UPDATE books SET scan_key = ? WHERE id = ?")
            .bind(&scan_key)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Fill `merged_uuids.scan_key` from the attached file's `book_files` row
/// (its location-override `path` + stem), so an attached file survives a
/// repoint of its scan root the same way a native file does.
async fn backfill_merged_scan_keys(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT mu.uuid, COALESCE(bf.path, ''), bf.filename, bf.format,
                (SELECT COUNT(*) FROM book_file_parts p WHERE p.book_file_id = bf.id)
           FROM merged_uuids mu
           JOIN book_files bf ON bf.book_id = mu.book_id AND bf.format = mu.format
          WHERE mu.scan_key IS NULL",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for (uuid, path, filename, format, part_count) in rows {
        let scan_key = reconstruct_scan_key(&path, &filename, &format, part_count);
        sqlx::query("UPDATE merged_uuids SET scan_key = ? WHERE uuid = ?")
            .bind(&scan_key)
            .bind(&uuid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
