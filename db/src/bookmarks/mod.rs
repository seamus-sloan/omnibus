//! Bookmark CRUD — create, list, update, and delete user bookmarks anchored
//! to an opaque position token within a book. One model serves both the
//! audiobook player (position = seconds) and the reader (position = EPUB CFI).

use omnibus_shared::{Bookmark, CreateBookmark};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

#[derive(Debug, thiserror::Error)]
pub enum BookmarkError {
    #[error("book not found")]
    BookNotFound,
    #[error("bookmark not found")]
    NotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for BookmarkError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Create a bookmark and return the persisted row.
pub async fn create_bookmark(
    pool: &SqlitePool,
    user_id: i64,
    input: &CreateBookmark,
) -> Result<Bookmark, BookmarkError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &input.book_uuid)
        .await?
        .ok_or(BookmarkError::BookNotFound)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO bookmarks (user_id, book_uuid, position, title)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(&input.position)
    .bind(input.title.as_deref())
    .fetch_one(pool)
    .await?;

    get_bookmark_by_id(pool, user_id, id).await
}

/// List all bookmarks for a user + book, ordered by creation time (oldest first).
pub async fn list_bookmarks(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Vec<Bookmark>, BookmarkError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(vec![]);
    };
    let rows = sqlx::query(
        "SELECT id, book_uuid, position, title, created_at
         FROM bookmarks
         WHERE user_id = ? AND book_uuid = ?
         ORDER BY created_at ASC",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_all(pool)
    .await?;

    rows.iter().map(row_to_bookmark).collect()
}

/// Set or clear the title/note on a bookmark owned by the user.
pub async fn update_bookmark(
    pool: &SqlitePool,
    user_id: i64,
    bookmark_id: i64,
    title: Option<&str>,
) -> Result<(), BookmarkError> {
    let result = sqlx::query("UPDATE bookmarks SET title = ? WHERE id = ? AND user_id = ?")
        .bind(title)
        .bind(bookmark_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(BookmarkError::NotFound);
    }
    Ok(())
}

/// Delete a bookmark by id, scoped to the owning user.
pub async fn delete_bookmark(
    pool: &SqlitePool,
    user_id: i64,
    bookmark_id: i64,
) -> Result<(), BookmarkError> {
    let result = sqlx::query("DELETE FROM bookmarks WHERE id = ? AND user_id = ?")
        .bind(bookmark_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(BookmarkError::NotFound);
    }
    Ok(())
}

async fn get_bookmark_by_id(
    pool: &SqlitePool,
    user_id: i64,
    bookmark_id: i64,
) -> Result<Bookmark, BookmarkError> {
    let row = sqlx::query(
        "SELECT id, book_uuid, position, title, created_at
         FROM bookmarks
         WHERE id = ? AND user_id = ?",
    )
    .bind(bookmark_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(BookmarkError::NotFound)?;

    row_to_bookmark(&row)
}

fn row_to_bookmark(row: &sqlx::sqlite::SqliteRow) -> Result<Bookmark, BookmarkError> {
    Ok(Bookmark {
        id: row.try_get("id")?,
        book_uuid: row.try_get::<String, _>("book_uuid")?,
        position: row.try_get("position")?,
        title: row.try_get("title")?,
        created_at: row.try_get("created_at")?,
    })
}

#[cfg(test)]
mod tests;
