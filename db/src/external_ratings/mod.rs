//! Storage for the community ratings the metadata providers publish — one row
//! per `(book, provider)`, upserted when a candidate is applied and read back
//! on the book-detail payload. Distinct from `crate::ratings`, which is the
//! reader's own star rating; the two never share a field.

use omnibus_shared::external_ratings::{ExternalRating, ProviderRating};
use omnibus_shared::metadata_lookup::MetadataProvider;
use sqlx::{Row, SqlitePool};

use crate::metadata_lookup::{fetch_all_ratings, MetadataLookupConfig};
use crate::resolve_canonical_book_uuid;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum ExternalRatingsError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for ExternalRatingsError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            // `resolve_canonical_book_uuid` is the only `BooksError`-returning
            // call this module makes and it decodes no JSON, so this variant
            // is unreachable; folded rather than panicked on, as in `ratings`.
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Refresh every configured provider's community rating for one book and
/// return what is now stored.
///
/// `isbn13` identifies the edition to ask about — the applied candidate's.
/// Providers are asked concurrently and best-effort: one that fails, isn't
/// configured, or has no score simply contributes no row, and its previously
/// stored row is left alone rather than deleted, since "we couldn't ask" is
/// not "the rating went away".
pub async fn refresh_ratings(
    pool: &SqlitePool,
    config: &MetadataLookupConfig,
    book_uuid: &str,
    isbn13: &str,
) -> Result<Vec<ExternalRating>, ExternalRatingsError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ExternalRatingsError::BookNotFound)?;

    // One transaction for the whole apply: a partial write would leave the
    // book showing some sources refreshed and others not, with nothing to say
    // which is which.
    let mut tx = pool.begin().await?;
    for (provider, rating) in fetch_all_ratings(config, isbn13).await {
        upsert_rating_for(&mut *tx, &canonical, provider, &rating).await?;
    }
    tx.commit().await?;
    list_ratings_for(pool, &canonical).await
}

/// Store one provider's rating for a book, replacing that provider's previous
/// row. Keyed `(book_uuid, provider)`, so re-applying a candidate updates in
/// place instead of accumulating a row per apply.
///
/// Stores against the **canonical** uuid, so a row written under a merged one
/// is still the row [`list_ratings`] reads back.
pub async fn upsert_rating(
    pool: &SqlitePool,
    book_uuid: &str,
    provider: MetadataProvider,
    rating: &ProviderRating,
) -> Result<(), ExternalRatingsError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ExternalRatingsError::BookNotFound)?;
    upsert_rating_for(pool, &canonical, provider, rating).await
}

/// [`upsert_rating`] for a uuid the caller has already canonicalized.
/// Executor-generic so [`refresh_ratings`] can run every provider's write in
/// one transaction; pass `&pool` for a standalone write.
async fn upsert_rating_for<'e, E>(
    executor: E,
    book_uuid: &str,
    provider: MetadataProvider,
    rating: &ProviderRating,
) -> Result<(), ExternalRatingsError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO book_external_ratings
             (book_uuid, provider, rating, rating_max, ratings_count, source_url, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?, strftime('%s','now'))
         ON CONFLICT(book_uuid, provider) DO UPDATE SET
             rating        = excluded.rating,
             rating_max    = excluded.rating_max,
             ratings_count = excluded.ratings_count,
             source_url    = excluded.source_url,
             fetched_at    = excluded.fetched_at",
    )
    .bind(book_uuid)
    .bind(provider.as_str())
    .bind(rating.rating)
    .bind(rating.rating_max)
    .bind(rating.ratings_count)
    .bind(rating.source_url.as_deref())
    .execute(executor)
    .await?;
    Ok(())
}

/// Every stored community rating for a book, newest-fetched first with the
/// provider token as the tiebreak (one apply stamps them the same second).
///
/// Resolves through `merged_uuids` like every other uuid read path, so a
/// merged or auto-attached uuid still finds the surviving book's ratings.
pub async fn list_ratings(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<ExternalRating>, ExternalRatingsError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(Vec::new());
    };
    list_ratings_for(pool, &canonical).await
}

/// [`list_ratings`] for a uuid the caller has already canonicalized.
async fn list_ratings_for(
    pool: &SqlitePool,
    canonical: &str,
) -> Result<Vec<ExternalRating>, ExternalRatingsError> {
    let rows = sqlx::query(
        "SELECT provider, rating, rating_max, ratings_count, source_url, fetched_at
           FROM book_external_ratings
          WHERE book_uuid = ?
          ORDER BY fetched_at DESC, provider ASC",
    )
    .bind(canonical)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().filter_map(row_to_rating).collect())
}

/// Map one stored row, back through the same `ProviderRating::new` gate the
/// write went through. A `provider` token no [`MetadataProvider`] variant
/// matches yields `None` rather than an error — a row written by a build that
/// knew a source this one doesn't is stale data, not a failed read.
fn row_to_rating(row: &sqlx::sqlite::SqliteRow) -> Option<ExternalRating> {
    let provider = MetadataProvider::from_str(&row.try_get::<String, _>("provider").ok()?)?;
    let rating = ProviderRating::new(
        row.try_get("rating").ok(),
        row.try_get("rating_max").ok()?,
        row.try_get("ratings_count").ok()?,
        row.try_get("source_url").ok()?,
    )?;
    Some(ExternalRating::new(
        provider,
        rating,
        row.try_get("fetched_at").ok()?,
    ))
}
