//! Per-user star-rating CRUD. One row per `(user, book)`, last-write-wins on the
//! value, soft-referencing the durable `books.uuid` (no FK/cascade) through the
//! merged-uuid-aware canonical resolver. A pruned book detaches its rating
//! rather than deleting it; the missing-files GC keeps any book a rating
//! references.

use omnibus_shared::{stars_from_half_stars, RatingRecord, RatingUpdate};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

#[derive(Debug, thiserror::Error)]
pub enum RatingError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for RatingError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            // `resolve_canonical_book_uuid` is the only `BooksError`-returning
            // call this module makes and it never decodes JSON, so the
            // `OverridesJson` variant is unreachable here. Fold it into a
            // decode error rather than panicking, mirroring `progress`.
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Upsert the rating for `(user, book)` and return the new server-authoritative
/// record. Resolves the request uuid to the **canonical** `books.uuid` (keeping
/// the `BookNotFound` guard — you cannot rate a book the server has never
/// indexed) and stores/keys on it.
pub async fn set_rating(
    pool: &SqlitePool,
    user_id: i64,
    update: &RatingUpdate,
) -> Result<RatingRecord, RatingError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &update.book_uuid)
        .await?
        .ok_or(RatingError::BookNotFound)?;
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, ?, strftime('%s','now'))
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET
             half_stars = excluded.half_stars,
             updated_at = strftime('%s','now')",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(update.half_stars())
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT half_stars, updated_at FROM user_ratings
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .fetch_one(pool)
    .await?;
    Ok(RatingRecord {
        book_uuid,
        stars: stars_from_half_stars(row.try_get::<i64, _>("half_stars")?),
        updated_at: row.try_get::<i64, _>("updated_at")?,
    })
}

/// Fetch the current rating for `(user, book_uuid)`. Returns `Ok(None)` for an
/// unknown book uuid or for a book the user has not rated yet.
pub async fn get_rating(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<RatingRecord>, RatingError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let Some(row) = sqlx::query(
        "SELECT half_stars, updated_at FROM user_ratings
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(RatingRecord {
        book_uuid: canonical,
        stars: stars_from_half_stars(row.try_get::<i64, _>("half_stars")?),
        updated_at: row.try_get::<i64, _>("updated_at")?,
    }))
}

/// Remove the current user's rating for a book (un-rate). A no-op when no row
/// exists — clearing an absent rating is not an error. Resolves the uuid the
/// same merged-uuid-aware way as the writes so a format-merged uuid clears the
/// surviving book's row.
pub async fn delete_rating(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<(), RatingError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(());
    };
    sqlx::query("DELETE FROM user_ratings WHERE user_id = ? AND book_uuid = ?")
        .bind(user_id)
        .bind(&canonical)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
