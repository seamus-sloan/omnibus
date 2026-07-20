//! The single door for maintaining the standalone `books_fts` index.
//!
//! `books_fts` is a standalone FTS5 vtable (no `content=`) with only
//! rename triggers, so every book mutation must mirror its row by hand.
//! Rather than open-code a DELETE/INSERT at each write site, all paths
//! route through [`upsert_fts`] / [`delete_fts`] here. Both take a
//! `&mut SqliteConnection` so they work inside a transaction
//! (`&mut **tx`) and on a pooled connection alike. The full-index repair
//! job [`rebuild_all_fts`] is built on the same door.

use std::sync::OnceLock;

use anyhow::Context;
use sqlx::{SqliteConnection, SqlitePool};

/// The `SELECT` projection shared by the per-book upsert and the
/// whole-table rebuild: same columns, same correlated subqueries for the
/// taxonomy joins and the ISBN lookup. Kept as one constant so the two
/// call sites can't drift apart — append `WHERE b.id = ?` for the
/// single-book form, or leave it off to project every row.
///
/// `authors` / `series` / `tags` mirror the rename-trigger projections in
/// 0005_fts5.sql so the inline upsert and the triggers agree on the same
/// text. `isbn` takes the first ISBN-scheme identifier (case-insensitive)
/// straight from `book_identifiers` — the canonical source now that the
/// denormalized `books.isbn` column is gone (F8).
const FTS_SELECT_FROM_BOOKS: &str = "
    SELECT
        b.id,
        COALESCE(b.title, ''),
        COALESCE((SELECT group_concat(a.name, ' ')
                  FROM books_authors_link l JOIN authors a ON a.id = l.author
                  WHERE l.book = b.id), ''),
        COALESCE((SELECT group_concat(s.name, ' ')
                  FROM books_series_link l JOIN series s ON s.id = l.series
                  WHERE l.book = b.id), ''),
        COALESCE((SELECT group_concat(t.name, ' ')
                  FROM books_tags_link l JOIN tags t ON t.id = l.tag
                  WHERE l.book = b.id), ''),
        COALESCE(b.description, ''),
        COALESCE((SELECT bi.value FROM book_identifiers bi
                  WHERE bi.book_id = b.id AND bi.scheme = 'ISBN' COLLATE NOCASE
                  LIMIT 1), '')
     FROM books b";

/// Delete-then-insert the `books_fts` row for `book_id`, sourcing the
/// indexed text from the canonical `books` row plus its taxonomy links.
///
/// This is the only place that inserts a single `books_fts` row (the
/// whole-table rebuild in [`rebuild_all_fts`] uses the same projection but
/// batches it into one statement). The column set (`title, authors,
/// series, tags, description, isbn`) is identical for ebooks and
/// audiobooks — both live in the same `books` table and the joins below
/// read whatever link rows exist — so one door serves both. A no-op
/// INSERT when `book_id` has no `books` row, but callers only pass live
/// ids.
pub(crate) async fn upsert_fts(
    conn: &mut SqliteConnection,
    book_id: i64,
) -> Result<(), sqlx::Error> {
    delete_fts(&mut *conn, book_id).await?;
    static UPSERT_SQL: OnceLock<String> = OnceLock::new();
    let sql = UPSERT_SQL.get_or_init(|| {
        format!(
            "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
             {FTS_SELECT_FROM_BOOKS}
             WHERE b.id = ?"
        )
    });
    sqlx::query(sql).bind(book_id).execute(&mut *conn).await?;
    Ok(())
}

/// Delete the `books_fts` row for `book_id`. Idempotent — a missing row
/// is a no-op. The only place that removes a `books_fts` row by id.
pub(crate) async fn delete_fts(
    conn: &mut SqliteConnection,
    book_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Rebuild the entire `books_fts` index from `books`: drop every row, then
/// re-insert every row in one batched `INSERT ... SELECT`. Used by the
/// admin "rebuild search index" job to repair any drift left by a failed
/// post-commit refresh. Idempotent — safe to re-run.
///
/// Two DML statements total regardless of library size — previously this
/// looped `upsert_fts` per book (2N statements for N books, one DELETE +
/// one INSERT each). Runs in a single transaction so the index is never
/// observed empty mid-rebuild. Orphan `books_fts` rows (rowid with no
/// `books` row) are swept by the leading `DELETE`.
pub async fn rebuild_all_fts(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("rebuild_all_fts: begin transaction")?;
    sqlx::query("DELETE FROM books_fts")
        .execute(&mut *tx)
        .await
        .context("rebuild_all_fts: clear books_fts")?;
    sqlx::query(&format!(
        "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
         {FTS_SELECT_FROM_BOOKS}"
    ))
    .execute(&mut *tx)
    .await
    .context("rebuild_all_fts: repopulate books_fts")?;
    tx.commit()
        .await
        .context("rebuild_all_fts: commit transaction")?;
    Ok(())
}

#[cfg(test)]
mod tests;
