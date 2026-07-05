//! Public per-book journal CRUD.
//!
//! Entries are public: [`list_journal_entries`] returns every user's entries for
//! a book (a shared reading log), attributed by author. Create/update/delete are
//! owner-scoped — a non-owner edit/delete is indistinguishable from a missing
//! row (`NotFound`). Bodies soft-reference the durable `books.uuid` (no
//! FK/cascade) through the merged-uuid-aware canonical resolver. Markdown is
//! rendered to sanitized HTML on read (see [`markdown`]).

use omnibus_shared::{CreateJournalEntry, JournalEntry, UpdateJournalEntry};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

pub mod markdown;

#[cfg(test)]
mod tests;

/// Columns + author join shared by every entry read. `user_id` surfaces as
/// `author_id`; `users.username` as `author_name`.
const SELECT_ENTRY: &str = "SELECT je.id, je.book_uuid, je.user_id AS author_id,
        u.username AS author_name, je.body_md, je.progress, je.created_at, je.updated_at
   FROM journal_entries je
   JOIN users u ON u.id = je.user_id";

/// Hard cap on how many entries `list_journal_entries` returns for a single
/// book. Matches `LIST_BOOKMARKS_LIMIT`/`LIST_HIGHLIGHTS_LIMIT` — a
/// defensive ceiling so a book with a pathological entry count can't produce
/// an unbounded REST response.
pub const LIST_JOURNAL_ENTRIES_LIMIT: i64 = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("book not found")]
    BookNotFound,
    #[error("journal entry not found")]
    NotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for JournalError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            // `resolve_canonical_book_uuid` is the only `BooksError`-returning
            // call this module makes and it never decodes JSON, so the
            // `OverridesJson` variant is unreachable here — fold it into a
            // decode error rather than panicking (mirrors `ratings`).
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Create a journal entry on a book and return the persisted, rendered row.
/// Resolves the request uuid to the canonical `books.uuid` (`BookNotFound` when
/// the server has never indexed it) and stores/keys on it.
pub async fn create_journal_entry(
    pool: &SqlitePool,
    user_id: i64,
    input: &CreateJournalEntry,
) -> Result<JournalEntry, JournalError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &input.book_uuid)
        .await?
        .ok_or(JournalError::BookNotFound)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(&input.body_md)
    .bind(input.progress.map(|p| p as i64))
    .fetch_one(pool)
    .await?;

    get_entry_by_id(pool, id).await
}

/// List every user's entries for a book, newest first. Returns an empty list —
/// not an error — for an unknown uuid or a book with no entries yet.
pub async fn list_journal_entries(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<JournalEntry>, JournalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(vec![]);
    };
    let rows = sqlx::query(&format!(
        "{SELECT_ENTRY} WHERE je.book_uuid = ? ORDER BY je.created_at DESC, je.id DESC LIMIT ?"
    ))
    .bind(&canonical)
    .bind(LIST_JOURNAL_ENTRIES_LIMIT)
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_entry).collect()
}

/// Edit an entry owned by `user_id`, returning the updated row. Errors with
/// `NotFound` when the id does not exist or belongs to another user (the two
/// cases are deliberately indistinguishable).
pub async fn update_journal_entry(
    pool: &SqlitePool,
    user_id: i64,
    entry_id: i64,
    input: &UpdateJournalEntry,
) -> Result<JournalEntry, JournalError> {
    let result = sqlx::query(
        "UPDATE journal_entries
            SET body_md = ?, progress = ?, updated_at = strftime('%s','now')
          WHERE id = ? AND user_id = ?",
    )
    .bind(&input.body_md)
    .bind(input.progress.map(|p| p as i64))
    .bind(entry_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(JournalError::NotFound);
    }
    get_entry_by_id(pool, entry_id).await
}

/// Delete an entry owned by `user_id`. Errors with `NotFound` when the id does
/// not exist or belongs to another user.
pub async fn delete_journal_entry(
    pool: &SqlitePool,
    user_id: i64,
    entry_id: i64,
) -> Result<(), JournalError> {
    let result = sqlx::query("DELETE FROM journal_entries WHERE id = ? AND user_id = ?")
        .bind(entry_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(JournalError::NotFound);
    }
    Ok(())
}

/// Re-read one entry by id (no user scope — callers have already authorized).
async fn get_entry_by_id(pool: &SqlitePool, entry_id: i64) -> Result<JournalEntry, JournalError> {
    let row = sqlx::query(&format!("{SELECT_ENTRY} WHERE je.id = ?"))
        .bind(entry_id)
        .fetch_optional(pool)
        .await?
        .ok_or(JournalError::NotFound)?;
    row_to_entry(&row)
}

/// Map a row to a `JournalEntry`, rendering the markdown body to sanitized HTML.
fn row_to_entry(row: &sqlx::sqlite::SqliteRow) -> Result<JournalEntry, JournalError> {
    let body_md: String = row.try_get("body_md")?;
    let body_html = markdown::render(&body_md);
    let progress: Option<i64> = row.try_get("progress")?;
    Ok(JournalEntry {
        id: row.try_get("id")?,
        book_uuid: row.try_get("book_uuid")?,
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        body_md,
        body_html,
        progress: progress.map(|p| p as u8),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
