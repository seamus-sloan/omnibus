//! Genre-cloud read: the global genre list with per-genre book counts.
//! Genres have no canonical link table (migration `0066`) — a book's genres
//! live only in `metadata_overrides.overrides -> '$.genres'` — so this is
//! the override arm of [`super::tags::get_tag_cloud`] on its own, with no
//! canonical arm to union against. A genre is visible only while at least
//! one live book's override still names it; a genre with zero books never
//! renders.

use omnibus_shared::GenreWeight;
use sqlx::{Row, SqlitePool};

use super::DiscoveryError;

/// Maximum number of genres returned from [`get_genre_cloud`]. Mirrors
/// `TAG_CLOUD_LIMIT` — a genre vocabulary is realistically far smaller, but
/// nothing stops a user from coining one per book.
const GENRE_CLOUD_LIMIT: i64 = 500;

/// Return up to [`GENRE_CLOUD_LIMIT`] genres with their book counts, ordered
/// by count descending then name ascending. Backs `/api/genres` and the
/// genre chip-editor's autocomplete pool.
///
/// Currently returns results across all users (single-tenant); scoping to a
/// per-user `user_id` is a future extension, not yet needed.
pub async fn get_genre_cloud(pool: &SqlitePool) -> Result<Vec<GenreWeight>, DiscoveryError> {
    // The join to `genres` — rather than grouping `je.value` directly — is
    // what makes the *display* name canonical: `materialize_genre_rows`
    // deduplicates into a `NOCASE`-unique row, so a book overridden with
    // "sci-fi" and another with "Sci-Fi" report as one genre under whichever
    // spelling was coined first. Grouping the raw JSON values instead would
    // split them into two entries and offer both in the autocomplete.
    //
    // `DISTINCT b.id` guards a duplicate entry inside a single book's array
    // (`["Horror","Horror"]` counts once), matching the tag cloud's `UNION`.
    let rows = sqlx::query(
        r#"SELECT g.name AS name, COUNT(DISTINCT b.id) AS cnt
             FROM genres g
             JOIN books b
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
             JOIN json_each(mo.overrides, '$.genres') je
            WHERE json_type(mo.overrides, '$.genres') IS NOT NULL
              AND je.value = g.name COLLATE NOCASE
            GROUP BY g.id, g.name
            ORDER BY cnt DESC, g.name ASC
            LIMIT ?"#,
    )
    .bind(GENRE_CLOUD_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| GenreWeight {
            name: r.get("name"),
            count: usize::try_from(r.get::<i64, _>("cnt")).unwrap_or(0),
        })
        .collect())
}
