//! Persist a completed conversion's output as a `book_files` row (#949), so
//! the converted format becomes a real, listed format of the book — visible
//! on the book detail page and to downloads — rather than an invisible file
//! sitting in the conversion cache. File-write shape mirrors
//! `crate::kepub::convert::convert_book`'s cache write; the DB write is new
//! (KEPUB is served from its own cache, never `book_files`).

use std::path::Path;

use sqlx::SqlitePool;

use super::fs::mtime_epoch;

/// `book_files.origin` value written by [`persist_converted_file`]. Existing
/// rows all have `origin IS NULL` (the scanner never sets it); this is what
/// the migration-0077 partial unique index scopes uniqueness to, so a
/// converted row can never collide with an unrelated scanned file that
/// happens to share the same format.
const CONVERTED_ORIGIN: &str = "converted";

/// Insert or refresh the `book_files` row for `book_id`'s conversion to
/// `target_format`, whose bytes now live at `out_path` — always
/// `super::fs::convert_path(book_id, target_format)`, the caller's own
/// output path from a just-completed [`super::execute::convert_book`].
///
/// Idempotent: a second conversion of the same `(book_id, target_format)`
/// updates the existing converted row's stat columns in place via
/// `ON CONFLICT` against the partial unique index scoped to
/// `origin = 'converted'` (migration 0077) — it never touches a *scanned*
/// file already occupying that format slot.
///
/// `library_path`/`path`/`filename` are set so the same reconstruction
/// `book_file_path` uses (`COALESCE(bf.library_path, l.path)` +
/// `COALESCE(bf.path, b.path)` + `filename.format`) resolves back to
/// exactly `out_path`: `library_path` is `out_path`'s parent (the convert
/// cache dir) as an absolute override, `path` is the empty string so the
/// join can't fall through to the book's own relative `books.path`, and
/// `filename` is the bare book id — matching `convert_path`'s
/// `<book_id>.<format>` naming.
pub(super) async fn persist_converted_file(
    pool: &SqlitePool,
    book_id: i64,
    target_format: &str,
    out_path: &Path,
) -> Result<(), sqlx::Error> {
    let meta = tokio::fs::metadata(out_path).await?;
    let size_bytes = meta.len() as i64;
    let mtime = mtime_epoch(&meta);
    let format = target_format.to_uppercase();
    let filename = book_id.to_string();
    let library_path = out_path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, library_path, path, origin)
         VALUES (?, ?, ?, ?, ?, ?, '', ?)
         ON CONFLICT(book_id, format) WHERE origin = 'converted' DO UPDATE SET
            filename     = excluded.filename,
            size_bytes   = excluded.size_bytes,
            mtime_epoch  = excluded.mtime_epoch,
            library_path = excluded.library_path,
            path         = excluded.path",
    )
    .bind(book_id)
    .bind(&format)
    .bind(&filename)
    .bind(size_bytes)
    .bind(mtime)
    .bind(&library_path)
    .bind(CONVERTED_ORIGIN)
    .execute(pool)
    .await?;

    Ok(())
}
