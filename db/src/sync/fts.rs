//! The single door for maintaining the standalone `books_fts` index. It is a
//! standalone FTS5 vtable (no `content=`) with only rename triggers, so every
//! book mutation mirrors its row by hand through [`upsert_fts`] /
//! [`delete_fts`] rather than an open-coded DELETE/INSERT per write site. Both
//! take a `&mut SqliteConnection`, so they work in a transaction or on a pool.

use std::sync::OnceLock;

use anyhow::Context;
use sqlx::{SqliteConnection, SqlitePool};

/// Rows per chunk for [`upsert_fts_batch`]'s two `IN (...)` lists — one bind
/// per book id per list, comfortably under SQLite's ~999-parameter cap.
const UPSERT_BATCH_CHUNK: usize = 450;

/// The `SELECT` projection shared by the per-book upsert and the
/// whole-table rebuild: same columns, same correlated subqueries for the
/// taxonomy joins and the ISBN lookup. Built once so the three call sites
/// can't drift apart — append `WHERE b.id = ?` for the single-book form, or
/// leave it off to project every row.
///
/// `authors` / `series` / `tags` mirror the rename-trigger projections in
/// 0005_fts5.sql so the inline upsert and the triggers agree on the same
/// text. `isbn` takes the first ISBN-scheme identifier (case-insensitive)
/// straight from `book_identifiers` — the canonical source now that the
/// denormalized `books.isbn` column is gone (F8). `genres` is the one column
/// with no canonical table behind it: nothing Omnibus parses carries a genre,
/// so migration `0066` gives them no link table and the override JSON is
/// their only storage — see [`GENRES_FROM_OVERRIDES`].
fn fts_select_from_books() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        format!(
            "
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
                  LIMIT 1), ''),
        {GENRES_FROM_OVERRIDES}
     FROM books b"
        )
    })
}

/// The `genres` source expression for a book aliased `b`: the override JSON's
/// `$.genres`, space-joined. Two guards, both load-bearing now that this runs
/// on the *write* path — every scan and every admin rebuild — rather than on
/// one read.
///
/// **`json_valid`, applied to the argument rather than as a `WHERE` filter.**
/// `json_each` raises `malformed JSON` on a bad blob, and a corrupt
/// `overrides` row is reachable state this codebase already models (a
/// hand-edited DB, or a schema predating a field — see
/// `get_metadata_overrides_returns_serialization_error_for_corrupt_blob`).
/// One such row would otherwise fail every reindex and the admin rebuild. A
/// `WHERE` guard would leave that riding on the planner evaluating it before
/// the table-valued function; substituting `'{}'` into the argument cannot.
///
/// **The precedence gate**, mirroring `apply_overrides`'s
/// `overrides_outrank_embedded` early return. Without it this door and
/// `overlay_overrides` — which sources the same column from precedence-gated
/// merged metadata — write different genres for the same book inside the same
/// transaction, so what `genre:` matched would depend on which path last
/// touched the row. A no-op under the default precedence, where the override
/// layer already outranks embedded metadata.
///
/// `json_each` on an array exposes the 0-based index as `key`, so the gate is
/// a direct translation of that function's `position()` comparison. A missing
/// entry on either side leaves the comparison NULL and falls back to true,
/// matching its `_ => true` arm — as does a malformed list, which
/// `parse_metadata_precedence` likewise resolves to the overrides-win default.
const GENRES_FROM_OVERRIDES: &str = "
    CASE WHEN (SELECT COALESCE(
                 (SELECT o.key FROM json_each(CASE WHEN json_valid(sr.metadata_precedence)
                                                   THEN sr.metadata_precedence ELSE '[]' END) o
                   WHERE o.value = 'omnibus_overrides')
                 >
                 (SELECT e.key FROM json_each(CASE WHEN json_valid(sr.metadata_precedence)
                                                   THEN sr.metadata_precedence ELSE '[]' END) e
                   WHERE e.value = 'embedded_tags'), 1)
                FROM scan_roots sr WHERE sr.id = b.library_id)
         THEN COALESCE((SELECT group_concat(je.value, ' ')
                          FROM metadata_overrides mo
                          JOIN json_each(CASE WHEN json_valid(mo.overrides)
                                              THEN mo.overrides ELSE '{}' END, '$.genres') je
                         WHERE mo.book_uuid = b.uuid), '')
         ELSE '' END";

/// The `books_fts` column list every INSERT site names, in
/// [`fts_select_from_books`]'s projection order. One constant so a future
/// column can't be added to the projection and left out of one of the three
/// insert sites.
const FTS_INSERT_COLUMNS: &str = "rowid, title, authors, series, tags, description, isbn, genres";

/// Delete-then-insert the `books_fts` row for `book_id`, sourcing the
/// indexed text from the canonical `books` row plus its taxonomy links.
///
/// This is the only place that inserts a single `books_fts` row (the
/// whole-table rebuild in [`rebuild_all_fts`] uses the same projection but
/// batches it into one statement). The column set
/// ([`FTS_INSERT_COLUMNS`]) is identical for ebooks and
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
        let select = fts_select_from_books();
        format!(
            "INSERT INTO books_fts({FTS_INSERT_COLUMNS})
             {select}
             WHERE b.id = ?"
        )
    });
    sqlx::query(sql).bind(book_id).execute(&mut *conn).await?;
    Ok(())
}

/// The many-books counterpart of [`upsert_fts`]: delete-then-insert the
/// `books_fts` rows for every id in `book_ids`, two DML statements per
/// chunk of [`UPSERT_BATCH_CHUNK`] rather than 2N statements for N books.
/// For a caller that already knows its whole affected-book set up front
/// (a merge/split undo replaying a snapshot) this replaces a per-book
/// `upsert_fts` loop; duplicate ids in `book_ids` are harmless — the
/// `INSERT ... SELECT ... WHERE b.id IN (...)` reads one row per distinct
/// book regardless of how many times its id appears in the bound list.
pub(crate) async fn upsert_fts_batch(
    conn: &mut SqliteConnection,
    book_ids: &[i64],
) -> Result<(), sqlx::Error> {
    for chunk in book_ids.chunks(UPSERT_BATCH_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        let del_sql = format!("DELETE FROM books_fts WHERE rowid IN ({placeholders})");
        let mut del_q = sqlx::query(&del_sql);
        for &id in chunk {
            del_q = del_q.bind(id);
        }
        del_q.execute(&mut *conn).await?;

        let select = fts_select_from_books();
        let ins_sql = format!(
            "INSERT INTO books_fts({FTS_INSERT_COLUMNS})
             {select}
             WHERE b.id IN ({placeholders})"
        );
        let mut ins_q = sqlx::query(&ins_sql);
        for &id in chunk {
            ins_q = ins_q.bind(id);
        }
        ins_q.execute(&mut *conn).await?;
    }
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
    let select = fts_select_from_books();
    sqlx::query(&format!(
        "INSERT INTO books_fts({FTS_INSERT_COLUMNS})
         {select}"
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
