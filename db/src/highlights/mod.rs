//! Highlight annotation CRUD — create, list, update, and delete
//! user highlights anchored to EPUB CFI ranges within a book.

use omnibus_shared::{CreateHighlight, Highlight, HighlightColor};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

#[derive(Debug, thiserror::Error)]
pub enum HighlightError {
    #[error("book not found")]
    BookNotFound,
    #[error("highlight not found")]
    NotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for HighlightError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Create a highlight and return the persisted row.
pub async fn create_highlight(
    pool: &SqlitePool,
    user_id: i64,
    input: &CreateHighlight,
) -> Result<Highlight, HighlightError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &input.book_uuid)
        .await?
        .ok_or(HighlightError::BookNotFound)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO highlights (user_id, book_uuid, epub_cfi_range, color)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(&input.epub_cfi_range)
    .bind(input.color.as_str())
    .fetch_one(pool)
    .await?;

    get_highlight_by_id(pool, user_id, id).await
}

/// List all highlights for a user + book, ordered by creation time.
pub async fn list_highlights(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Vec<Highlight>, HighlightError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(vec![]);
    };
    let rows = sqlx::query(
        "SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.created_at
         FROM highlights h
         WHERE h.user_id = ? AND h.book_uuid = ?
         ORDER BY h.created_at ASC",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_all(pool)
    .await?;

    rows.iter().map(row_to_highlight).collect()
}

/// Change the color of an existing highlight.
pub async fn update_highlight_color(
    pool: &SqlitePool,
    user_id: i64,
    highlight_id: i64,
    color: HighlightColor,
) -> Result<(), HighlightError> {
    let result = sqlx::query("UPDATE highlights SET color = ? WHERE id = ? AND user_id = ?")
        .bind(color.as_str())
        .bind(highlight_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(HighlightError::NotFound);
    }
    Ok(())
}

/// Set or clear the note text on a highlight.
pub async fn update_highlight_note(
    pool: &SqlitePool,
    user_id: i64,
    highlight_id: i64,
    note: Option<&str>,
) -> Result<(), HighlightError> {
    let result = sqlx::query("UPDATE highlights SET note = ? WHERE id = ? AND user_id = ?")
        .bind(note)
        .bind(highlight_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(HighlightError::NotFound);
    }
    Ok(())
}

/// Delete a highlight by id, scoped to the owning user.
pub async fn delete_highlight(
    pool: &SqlitePool,
    user_id: i64,
    highlight_id: i64,
) -> Result<(), HighlightError> {
    let result = sqlx::query("DELETE FROM highlights WHERE id = ? AND user_id = ?")
        .bind(highlight_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(HighlightError::NotFound);
    }
    Ok(())
}

async fn get_highlight_by_id(
    pool: &SqlitePool,
    user_id: i64,
    highlight_id: i64,
) -> Result<Highlight, HighlightError> {
    let row = sqlx::query(
        "SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.created_at
         FROM highlights h
         WHERE h.id = ? AND h.user_id = ?",
    )
    .bind(highlight_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(HighlightError::NotFound)?;

    row_to_highlight(&row)
}

fn row_to_highlight(row: &sqlx::sqlite::SqliteRow) -> Result<Highlight, HighlightError> {
    let color_str: String = row.try_get("color")?;
    let color = HighlightColor::parse(&color_str).unwrap_or(HighlightColor::Amber);
    Ok(Highlight {
        id: row.try_get("id")?,
        book_uuid: row.try_get::<String, _>("book_uuid")?,
        epub_cfi_range: row.try_get("epub_cfi_range")?,
        color,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

#[cfg(test)]
mod tests;
