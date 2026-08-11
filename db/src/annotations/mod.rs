//! Annotation CRUD over the unified `annotations` table: web/iOS highlights
//! and notes (CFI-anchored) plus Kobo device annotations (KoboSpan-anchored,
//! held opaquely in `kobo_location`). The web wire surface keeps its
//! highlight naming; the Kobo Reading Services channel uses the
//! `*_kobo_annotations` functions.

use omnibus_shared::{CreateHighlight, Highlight, HighlightColor};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::resolve_canonical_book_uuid;

mod backfill;
mod downsync;

pub use backfill::{backfill_kobo_annotation_cfis, BackfillStats};
pub use downsync::{
    downsync_all_kobo_annotations, downsync_book_annotations, downsync_book_id_annotations,
    spawn_kobo_downsync, DownsyncStats,
};

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
/// capped by the server's tolerant PATCH parser; `epub_cfi_range` is the
/// server-derived web anchor (`kobo_position::annotation_cfis`), `None`
/// whenever derivation wasn't possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestKoboAnnotation {
    pub client_id: String,
    pub color: HighlightColor,
    pub text: Option<String>,
    pub note: Option<String>,
    pub kobo_location: String,
    pub epub_cfi_range: Option<String>,
}

/// List the Kobo-placeable annotations for a user + book — rows that carry a
/// `kobo_location` anchor: device-origin rows always, plus web-origin rows the
/// downsync pass has materialized (see [`downsync_book_annotations`]).
/// Unknown book resolves to an empty list, mirroring [`list_highlights`].
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

/// Batched counterpart to [`served_kobo_annotations`]: fetch the
/// Kobo-placeable annotations for every uuid in `book_uuids` in a single
/// query, grouped by `book_uuid`, instead of one round trip per uuid.
/// Mirrors [`crate::kobo::reading_state_for`]'s `IN (...)` shape and the
/// windowed per-group cap used by `covers_manual_batch`
/// ([`crate::shelves::read`]): a `ROW_NUMBER() OVER (PARTITION BY book_uuid
/// ORDER BY created_at ASC)` keeps each book's first [`LIST_HIGHLIGHTS_LIMIT`]
/// rows — the same rows and order [`served_kobo_annotations`] would return
/// for that book — so a caller comparing this batch's fingerprint against one
/// acked from the single-uuid path sees the same content. Chunked at 900 to
/// stay under SQLite's 999 bind-parameter cap (one bind per uuid plus
/// `user_id`). A uuid with nothing servable is absent from the map rather
/// than present with an empty vec. Unlike the single-uuid form, this does not
/// canonicalize `book_uuids` first — callers (today, `changed_book_uuids`)
/// must pass already-canonical uuids.
pub async fn served_kobo_annotations_batch(
    pool: &SqlitePool,
    user_id: i64,
    book_uuids: &[String],
) -> Result<std::collections::HashMap<String, Vec<ServedKoboAnnotation>>, HighlightError> {
    let mut out: std::collections::HashMap<String, Vec<ServedKoboAnnotation>> =
        std::collections::HashMap::with_capacity(book_uuids.len());
    if book_uuids.is_empty() {
        return Ok(out);
    }
    for chunk in book_uuids.chunks(900) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT book_uuid, client_id, color, text, note, kobo_location, updated_at
               FROM (
                 SELECT book_uuid, client_id, color, text, note, kobo_location, updated_at,
                        ROW_NUMBER() OVER (
                            PARTITION BY book_uuid ORDER BY created_at ASC
                        ) AS rn
                   FROM annotations
                  WHERE user_id = ? AND book_uuid IN ({placeholders})
                    AND kobo_location IS NOT NULL AND client_id IS NOT NULL
               )
              WHERE rn <= {LIST_HIGHLIGHTS_LIMIT}
              ORDER BY book_uuid, rn"
        );
        let mut q = sqlx::query(&sql).bind(user_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        for row in q.fetch_all(pool).await? {
            let color_str: String = row.try_get("color")?;
            let book_uuid: String = row.try_get("book_uuid")?;
            out.entry(book_uuid)
                .or_default()
                .push(ServedKoboAnnotation {
                    client_id: row.try_get("client_id")?,
                    color: HighlightColor::parse(&color_str).unwrap_or(HighlightColor::Amber),
                    text: row.try_get("text")?,
                    note: row.try_get("note")?,
                    kobo_location: row.try_get("kobo_location")?,
                    updated_at: row.try_get("updated_at")?,
                });
        }
    }
    Ok(out)
}

/// Bound-parameter-safe chunk size for the upsert VALUES batch: 8 binds per
/// row keeps each chunked statement's total well under SQLite's ~999
/// bound-parameter cap (120 * 8 = 960), mirroring the chunking pattern in
/// `metadata_overrides::upsert::load_overrides_bulk`.
const UPSERT_CHUNK_SIZE: usize = 120;

/// Bound-parameter-safe chunk size for the delete `IN (...)` batch: one bind
/// for `user_id` plus one per client_id keeps each chunked statement well
/// under SQLite's ~999 bound-parameter cap.
const DELETE_CHUNK_SIZE: usize = 500;

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
    upsert_kobo_annotations(&mut tx, user_id, &canonical, updates).await?;
    delete_kobo_annotations(&mut tx, user_id, deleted_client_ids).await?;
    tx.commit().await.map_err(HighlightError::Sqlx)?;
    Ok(())
}

/// Chunked multi-row upsert for a device PATCH's `updates` batch, in place of
/// one `INSERT ... ON CONFLICT` per row.
async fn upsert_kobo_annotations(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    canonical_book_uuid: &str,
    updates: &[IngestKoboAnnotation],
) -> Result<(), HighlightError> {
    for chunk in updates.chunks(UPSERT_CHUNK_SIZE) {
        let rows = std::iter::repeat_n("(?, ?, ?, ?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        // The CFI conflict rule keeps the stored web anchor truthful: a
        // moved kobo_location takes the fresh derivation even when it's
        // NULL (a stale CFI is a wrong location), while an unmoved anchor
        // keeps an existing CFI over a derivation that merely failed. Each
        // conflicting row in the batch resolves this independently against
        // its own `excluded.*` values.
        let sql = format!(
            "INSERT INTO annotations
                 (user_id, book_uuid, epub_cfi_range, kobo_location, color, note, text, client_id)
             VALUES {rows}
             ON CONFLICT(user_id, client_id) WHERE client_id IS NOT NULL DO UPDATE SET
                 epub_cfi_range = CASE
                     WHEN annotations.kobo_location IS NOT excluded.kobo_location
                         THEN excluded.epub_cfi_range
                     ELSE COALESCE(excluded.epub_cfi_range, annotations.epub_cfi_range)
                 END,
                 kobo_location = excluded.kobo_location,
                 color = excluded.color,
                 note = excluded.note,
                 text = excluded.text,
                 updated_at = strftime('%s','now')"
        );
        let mut q = sqlx::query(&sql);
        for a in chunk {
            q = q
                .bind(user_id)
                .bind(canonical_book_uuid)
                .bind(a.epub_cfi_range.as_deref())
                .bind(&a.kobo_location)
                .bind(a.color.as_str())
                .bind(a.note.as_deref())
                .bind(a.text.as_deref())
                .bind(&a.client_id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Chunked `DELETE ... WHERE client_id IN (...)` for a device PATCH's
/// `deleted_client_ids` batch, in place of one `DELETE` per row.
async fn delete_kobo_annotations(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    deleted_client_ids: &[String],
) -> Result<(), HighlightError> {
    for chunk in deleted_client_ids.chunks(DELETE_CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("DELETE FROM annotations WHERE user_id = ? AND client_id IN ({placeholders})");
        let mut q = sqlx::query(&sql).bind(user_id);
        for client_id in chunk {
            q = q.bind(client_id);
        }
        q.execute(&mut **tx).await?;
    }
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
