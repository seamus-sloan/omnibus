//! Materialize the side tables an override touches so downstream reads
//! still resolve. The series-name override gets a canonical row + link; the
//! subjects (tags) and genres overrides get a vocabulary row only — their
//! memberships stay override-JSON-side (see [`materialize_tag_rows`]). The
//! author override path replaces its m2m list at read time via
//! `apply_overrides` and doesn't need a helper here.

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

/// When an override sets a subjects list, ensure a `tags` row exists for
/// every tag in it, so `get_tag_cloud` — which feeds the `/tags` cloud and
/// the inline-edit autocomplete pool — has a canonical row to surface a
/// tag created through the override path.
///
/// Deliberately rows-only, **not** `books_tags_link` rows: the canonical
/// link table is the sole record of a book's *scanned* tags, so writing
/// override memberships into it would make "delete overrides → revert to
/// scanned" impossible. Memberships stay in the override JSON, which
/// `get_tag_cloud` already counts via `json_each`. The orphan sweep
/// (`delete_orphan_tags`) counts those memberships too, so a materialized
/// row lives exactly as long as a canonical link or a live book's override
/// still names it — and is deleted the moment neither does.
pub(super) async fn materialize_tag_rows(
    conn: &mut SqliteConnection,
    overrides: &MetadataOverrides,
) -> Result<(), sqlx::Error> {
    let Some(subjects) = overrides.subjects.as_ref() else {
        return Ok(());
    };
    for tag in subjects.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
            .bind(tag)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// When an override sets a genres list, ensure a `genres` row exists for
/// every entry, so `get_genre_cloud` — which feeds the `/api/genres` read
/// and the chip-editor autocomplete pool — can surface a genre the moment a
/// user coins it.
///
/// Rows-only, like [`materialize_tag_rows`], but for a different reason:
/// tags keep memberships override-side to preserve revert-to-scanned, while
/// genres have no scanned baseline at all (migration `0066`), so the
/// override JSON is simply their only storage. The orphan sweep
/// (`delete_orphan_genres`) reaps a row the moment no live book's override
/// still names it.
pub(super) async fn materialize_genre_rows(
    conn: &mut SqliteConnection,
    overrides: &MetadataOverrides,
) -> Result<(), sqlx::Error> {
    let Some(genres) = overrides.genres.as_ref() else {
        return Ok(());
    };
    for genre in genres.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        sqlx::query("INSERT OR IGNORE INTO genres (name) VALUES (?)")
            .bind(genre)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
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
