//! Materialize the side tables an override touches so downstream reads
//! still resolve. Today only the series-name override has a link to
//! materialize; the author/tag override path replaces those m2m lists at
//! read time via `apply_overrides` and doesn't yet need a sibling helper
//! here.

use omnibus_shared::MetadataOverrides;
use sqlx::SqliteConnection;

/// When an override sets a series name, ensure a `series` row and
/// `books_series_link` exist so the browse index and detail-page breadcrumb
/// resolve. Without this, override-only series are invisible to
/// `list_series` (which requires a canonical link for visibility) and the
/// book detail page's `series_id` backfill can't find the `series.id`.
///
/// Takes `&mut SqliteConnection` so the caller can run the four statements
/// (book lookup, series upsert, series read, link insert) inside an open
/// transaction — pass `&mut tx` from a `sqlx::Transaction`. Running
/// outside a transaction would let each statement commit independently
/// in autocommit mode, reopening the partial-failure window this helper
/// exists to close.
pub(super) async fn materialize_series_link(
    conn: &mut SqliteConnection,
    book_uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<(), sqlx::Error> {
    let series_name = overrides
        .series
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(series_name) = series_name else {
        return Ok(());
    };
    let Some(book_id) = lookup_book_id(&mut *conn, book_uuid).await? else {
        return Ok(());
    };
    let series_id = find_or_create_series(&mut *conn, series_name).await?;
    link_book_to_series(&mut *conn, book_id, series_id).await
}

/// Resolve `books.uuid` → `books.id`. Returns `None` if the row was
/// deleted out from under the override write path.
async fn lookup_book_id(
    conn: &mut SqliteConnection,
    book_uuid: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_optional(conn)
        .await
}

/// Idempotent series-row materialization: INSERT-OR-IGNORE the name, then
/// SELECT the id case-insensitively so the read matches the same NOCASE
/// uniqueness the canonical scan path uses.
async fn find_or_create_series(
    conn: &mut SqliteConnection,
    series_name: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO series (name) VALUES (?)")
        .bind(series_name)
        .execute(&mut *conn)
        .await?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ? COLLATE NOCASE")
        .bind(series_name)
        .fetch_one(conn)
        .await
}

/// Idempotent `books_series_link` insert — repeated overrides on the
/// same (book, series) pair are a no-op.
async fn link_book_to_series(
    conn: &mut SqliteConnection,
    book_id: i64,
    series_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
        .bind(book_id)
        .bind(series_id)
        .execute(conn)
        .await?;
    Ok(())
}
