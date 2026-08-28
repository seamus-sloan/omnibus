//! Genre rollups for the stats page's "What you read" donut: the per-genre
//! book counts its slices are drawn from, and the size of the population
//! those slices describe. Split from `compute.rs` to keep that file under the
//! house line-count cap.

use omnibus_shared::GenreShare;
use sqlx::{Row, SqlitePool};

use super::compute::SESSION_BOOK_SECS;
use super::StatsError;

/// Genre share by distinct book count: for each genre, how many distinct
/// books carrying it had session activity in the window. Count-based (not
/// seconds) so a multi-genre book counts once per genre but never twice per
/// genre.
///
/// Returns **every** genre, not a top-N: the donut folds everything past its
/// fourth slice into "Other" and can only size that fold honestly if given the
/// whole tail. Genres are a user-curated vocabulary (`0066_genres.sql`), so the
/// row count is bounded by what a reader has assigned.
///
/// Reads the user-assigned genres, which live only in the
/// `metadata_overrides` JSON blob (the table itself predates them, from
/// `0007`; `0066_genres.sql` adds the vocabulary table this joins for the
/// canonical spelling). This deliberately does *not* fall back to a book's
/// tags: the donut is labelled "What you read", and a `<dc:subject>` list is
/// whatever the publisher's OPF happened to carry, not a genre. A library
/// with no genres assigned yet renders an empty donut, which is the honest
/// answer.
pub(super) async fn genre_share(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<GenreShare>, StatsError> {
    // Collapse the per-session union to distinct active books *before* the
    // genre join, so the intermediate is bounded by book count, not session
    // count — keeps the join small on a 10k-event library.
    //
    // Joining `genres` rather than grouping `je.value` folds case variants
    // into the one canonical row, exactly as `get_genre_cloud` does — a
    // donut that split "sci-fi" from "Sci-Fi" would double-count a slice.
    let sql = format!(
        "SELECT g.name AS name, COUNT(DISTINCT b.uuid) AS books
             FROM (SELECT DISTINCT book_uuid FROM ({SESSION_BOOK_SECS})) x
             JOIN books b ON b.uuid = x.book_uuid
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
             JOIN json_each(mo.overrides, '$.genres') je
             JOIN genres g ON g.name = je.value COLLATE NOCASE
            WHERE json_type(mo.overrides, '$.genres') IS NOT NULL
         GROUP BY g.id ORDER BY books DESC, g.name ASC"
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| GenreShare {
            name: r.get("name"),
            books: r.get("books"),
        })
        .collect())
}

/// Distinct books with a genre *and* session activity in the window — the
/// population the donut's slices are drawn from, and so the number its center
/// reports.
///
/// Not [`super::compute::books_active`]: a book with no genre reaches that
/// count but no slice, so it overstates what the ring describes. Counted
/// distinctly, so a book carrying three genres counts once here even though it
/// appears in three slices.
pub(super) async fn genre_tagged_books(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COUNT(DISTINCT b.uuid)
             FROM (SELECT DISTINCT book_uuid FROM ({SESSION_BOOK_SECS})) x
             JOIN books b ON b.uuid = x.book_uuid
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
             JOIN json_each(mo.overrides, '$.genres') je
             JOIN genres g ON g.name = je.value COLLATE NOCASE
            WHERE json_type(mo.overrides, '$.genres') IS NOT NULL"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}
