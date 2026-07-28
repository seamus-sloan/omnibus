//! Annotation CRUD over the unified `annotations` table: web/iOS highlights
//! and notes (CFI-anchored) plus Kobo device annotations (KoboSpan-anchored,
//! held opaquely in `kobo_location`). The web wire surface keeps its
//! highlight naming; the Kobo Reading Services channel uses the
//! `*_kobo_annotations` functions.

use omnibus_shared::{CreateHighlight, Highlight, HighlightColor};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

/// Hard cap on how many highlights `list_highlights` returns for a single
/// `(user, book)` pair. Practical usage stays well under this; the cap
/// exists so a pathological or automated annotation pipeline can't
/// produce an unbounded REST response.
pub const LIST_HIGHLIGHTS_LIMIT: i64 = 1_000;

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
///
/// Idempotent when `input.client_id` is set: a replayed create (the mobile
/// outbox retrying an op whose response never made it back) resolves to the
/// same row rather than a duplicate, and returns it unchanged.
pub async fn create_highlight(
    pool: &SqlitePool,
    user_id: i64,
    input: &CreateHighlight,
) -> Result<Highlight, HighlightError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &input.book_uuid)
        .await?
        .ok_or(HighlightError::BookNotFound)?;
    let client_id = input.client_id.as_deref();
    // `DO NOTHING` rather than `DO UPDATE`: the first write of a given
    // client_id is the user's gesture, and a replay carries no newer intent.
    // Colour and note changes arrive as their own ops.
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO annotations (user_id, book_uuid, epub_cfi_range, color, text, client_id)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id, client_id) WHERE client_id IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(&input.epub_cfi_range)
    .bind(input.color.as_str())
    .bind(input.text.as_deref())
    .bind(client_id)
    .fetch_optional(pool)
    .await?;

    match (id, client_id) {
        (Some(id), _) => get_highlight_by_id(pool, user_id, id).await,
        // The insert was a no-op, so this client_id is already ours.
        (None, Some(client_id)) => get_highlight_by_client_id(pool, user_id, client_id).await,
        (None, None) => Err(HighlightError::NotFound),
    }
}

/// Resolve a client-minted id to the numeric row id, scoped to the owner.
/// `Ok(None)` when this user has no highlight under that handle.
pub async fn highlight_id_for_client_id(
    pool: &SqlitePool,
    user_id: i64,
    client_id: &str,
) -> Result<Option<i64>, HighlightError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM annotations WHERE user_id = ? AND client_id = ?",
    )
    .bind(user_id)
    .bind(client_id)
    .fetch_optional(pool)
    .await?)
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
        "SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.text,
                h.client_id, h.created_at
         FROM annotations h
         WHERE h.user_id = ? AND h.book_uuid = ?
         ORDER BY h.created_at ASC
         LIMIT ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(LIST_HIGHLIGHTS_LIMIT)
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
    let result = sqlx::query(
        "UPDATE annotations SET color = ?, updated_at = strftime('%s','now')
         WHERE id = ? AND user_id = ?",
    )
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
    let result = sqlx::query(
        "UPDATE annotations SET note = ?, updated_at = strftime('%s','now')
         WHERE id = ? AND user_id = ?",
    )
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
    let result = sqlx::query("DELETE FROM annotations WHERE id = ? AND user_id = ?")
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
        "SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.text,
                h.client_id, h.created_at
         FROM annotations h
         WHERE h.id = ? AND h.user_id = ?",
    )
    .bind(highlight_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(HighlightError::NotFound)?;

    row_to_highlight(&row)
}

async fn get_highlight_by_client_id(
    pool: &SqlitePool,
    user_id: i64,
    client_id: &str,
) -> Result<Highlight, HighlightError> {
    let row = sqlx::query(
        "SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.text,
                h.client_id, h.created_at
         FROM annotations h
         WHERE h.client_id = ? AND h.user_id = ?",
    )
    .bind(client_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(HighlightError::NotFound)?;

    row_to_highlight(&row)
}

/// A Kobo-placeable annotation as the Reading Services channel serves it:
/// the device-minted id rides in `client_id`, the anchor is the verbatim
/// `kobo_location` JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedKoboAnnotation {
    pub client_id: String,
    pub color: HighlightColor,
    pub text: Option<String>,
    pub note: Option<String>,
    pub kobo_location: String,
    pub updated_at: i64,
}

/// One device-uploaded annotation to persist. Fields are pre-validated and
/// capped by the server's tolerant PATCH parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestKoboAnnotation {
    pub client_id: String,
    pub color: HighlightColor,
    pub text: Option<String>,
    pub note: Option<String>,
    pub kobo_location: String,
}

/// List the Kobo-placeable annotations for a user + book — rows that carry a
/// `kobo_location` anchor (today that means device-origin rows; a future
/// CFI→KoboSpan converter widens the set with no query change). Unknown book
/// resolves to an empty list, mirroring [`list_highlights`].
pub async fn served_kobo_annotations(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Vec<ServedKoboAnnotation>, HighlightError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(vec![]);
    };
    let rows = sqlx::query(
        "SELECT client_id, color, text, note, kobo_location, updated_at
         FROM annotations
         WHERE user_id = ? AND book_uuid = ?
           AND kobo_location IS NOT NULL AND client_id IS NOT NULL
         ORDER BY created_at ASC
         LIMIT ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(LIST_HIGHLIGHTS_LIMIT)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            let color_str: String = row.try_get("color")?;
            Ok(ServedKoboAnnotation {
                client_id: row.try_get("client_id")?,
                color: HighlightColor::parse(&color_str).unwrap_or(HighlightColor::Amber),
                text: row.try_get("text")?,
                note: row.try_get("note")?,
                kobo_location: row.try_get("kobo_location")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect()
}

/// Apply one device PATCH: upsert `updates` (keyed on the device-minted id in
/// `client_id` — a re-upload updates color/note/text/anchor in place, so
/// replays are idempotent) and delete `deleted_client_ids`, in one
/// transaction. Unlike the web path's create, a replayed upload *does* carry
/// newer intent — the device batches edits into the same PATCH shape.
pub async fn ingest_kobo_annotations(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    updates: &[IngestKoboAnnotation],
    deleted_client_ids: &[String],
) -> Result<(), HighlightError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(HighlightError::BookNotFound)?;

    let mut tx = pool.begin().await.map_err(HighlightError::Sqlx)?;
    for a in updates {
        sqlx::query(
            "INSERT INTO annotations
                 (user_id, book_uuid, epub_cfi_range, kobo_location, color, note, text, client_id)
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, client_id) WHERE client_id IS NOT NULL DO UPDATE SET
                 kobo_location = excluded.kobo_location,
                 color = excluded.color,
                 note = excluded.note,
                 text = excluded.text,
                 updated_at = strftime('%s','now')",
        )
        .bind(user_id)
        .bind(&canonical)
        .bind(&a.kobo_location)
        .bind(a.color.as_str())
        .bind(a.note.as_deref())
        .bind(a.text.as_deref())
        .bind(&a.client_id)
        .execute(&mut *tx)
        .await?;
    }
    for client_id in deleted_client_ids {
        sqlx::query("DELETE FROM annotations WHERE user_id = ? AND client_id = ?")
            .bind(user_id)
            .bind(client_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await.map_err(HighlightError::Sqlx)?;
    Ok(())
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
        text: row.try_get("text")?,
        client_id: row.try_get("client_id")?,
        created_at: row.try_get("created_at")?,
    })
}

#[cfg(test)]
mod tests;
