//! Boot backfill for `books.scan_key` on rows indexed before it existed.
//!
//! Fills `scan_key IS NULL` rows purely from stored columns (no filesystem
//! reads) — idempotent, safe on in-memory test DBs. Mirrors
//! [`crate::normalize::backfill_norm_columns`]; runs from `init_db`.

use sqlx::SqlitePool;

/// Errors returned by [`backfill_scan_keys`].
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

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

/// One-time backfill of `books.scan_key` + `merged_uuids.scan_key` +
/// `book_files.scan_key` for rows indexed before migrations 0026 / 0043.
/// Idempotent — only touches rows where `scan_key IS NULL` — and pure DB
/// work, so it runs on every boot from `init_db` and against in-memory test
/// DBs.
pub async fn backfill_scan_keys(pool: &SqlitePool) -> Result<(), IdentityError> {
    backfill_books_scan_keys(pool).await?;
    backfill_merged_scan_keys(pool).await?;
    backfill_book_files_scan_keys(pool).await?;
    Ok(())
}

/// Fill `book_files.scan_key` (migration 0043) for every row from its own
/// stored `(path, filename, format)` + part count — the same reconstruction
/// the `books`/`merged_uuids` backfills use. Attached rows carry their
/// file's real `path` (the attach writer sets it) and reconstruct from it
/// directly. A **native** row never has `book_files.path` set (only
/// `books.path` is) — `bf.path` is `NULL` there, so this JOINs `books` and
/// falls back to the book's own `path` for those rows. Without the JOIN a
/// native row at `A/one.epub` would backfill to the bare leaf `one.epub`,
/// losing the directory — which broke the per-file `bf.scan_key = b.scan_key`
/// anchor match `list_indexed_rows_for_formats` relies on (#1537).
/// Idempotent — `scan_key IS NULL` only.
async fn backfill_book_files_scan_keys(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let rows: Vec<(i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT bf.id, COALESCE(bf.path, b.path), bf.filename, bf.format,
                (SELECT COUNT(*) FROM book_file_parts p WHERE p.book_file_id = bf.id)
           FROM book_files bf
           JOIN books b ON b.id = bf.book_id
          WHERE bf.scan_key IS NULL",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }

    let updates: Vec<(i64, String)> = rows
        .into_iter()
        .map(|(id, path, filename, format, part_count)| {
            (
                id,
                reconstruct_scan_key(&path, &filename, &format, part_count),
            )
        })
        .collect();

    let mut tx = pool.begin().await?;
    for chunk in updates.chunks(SCAN_KEY_UPDATE_CHUNK) {
        let values = std::iter::repeat_n("(?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE book_files SET scan_key = v.column2
               FROM (VALUES {values}) AS v
              WHERE book_files.id = v.column1"
        );
        let mut q = sqlx::query(&sql);
        for (id, scan_key) in chunk {
            q = q.bind(id).bind(scan_key);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Fill `books.scan_key` from each book's **native** `book_files` row — the
/// format that is *not* recorded in `merged_uuids` (attachments keep their
/// own key). A fileless book has no `book_files` row, but fileless rows are a
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

    // A book with two native formats would appear twice; the first
    // reconstruction wins (any native file locates the same book).
    let mut seen = std::collections::HashSet::new();
    let updates: Vec<(i64, String)> = rows
        .into_iter()
        .filter(|(id, ..)| seen.insert(*id))
        .map(|(id, path, filename, format, part_count)| {
            (
                id,
                reconstruct_scan_key(&path, &filename, &format, part_count),
            )
        })
        .collect();

    let mut tx = pool.begin().await?;
    for chunk in updates.chunks(SCAN_KEY_UPDATE_CHUNK) {
        let values = std::iter::repeat_n("(?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE books SET scan_key = v.column2
               FROM (VALUES {values}) AS v
              WHERE books.id = v.column1"
        );
        let mut q = sqlx::query(&sql);
        for (id, scan_key) in chunk {
            q = q.bind(id).bind(scan_key);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Rows per chunk for the `scan_key` backfill UPDATEs. Two binds per row keeps
/// a chunk well under SQLite's 999-parameter cap.
const SCAN_KEY_UPDATE_CHUNK: usize = 400;

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

    let updates: Vec<(String, String)> = rows
        .into_iter()
        .map(|(uuid, path, filename, format, part_count)| {
            (
                uuid,
                reconstruct_scan_key(&path, &filename, &format, part_count),
            )
        })
        .collect();

    let mut tx = pool.begin().await?;
    for chunk in updates.chunks(SCAN_KEY_UPDATE_CHUNK) {
        let values = std::iter::repeat_n("(?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE merged_uuids SET scan_key = v.column2
               FROM (VALUES {values}) AS v
              WHERE merged_uuids.uuid = v.column1"
        );
        let mut q = sqlx::query(&sql);
        for (uuid, scan_key) in chunk {
            q = q.bind(uuid).bind(scan_key);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
