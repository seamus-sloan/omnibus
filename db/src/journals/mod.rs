//! Public per-book journal CRUD.
//!
//! Published entries are public: [`list_journal_entries`] returns every user's
//! published entries for a book (a shared reading log), attributed by author,
//! plus the viewer's own drafts. Create/update/delete are
//! owner-scoped — a non-owner edit/delete is indistinguishable from a missing
//! row (`NotFound`). Bodies soft-reference the durable `books.uuid` (no
//! FK/cascade) through the merged-uuid-aware canonical resolver. Markdown is
//! rendered to sanitized HTML on read (see [`markdown`]).

use std::collections::HashSet;

use omnibus_shared::{CreateJournalEntry, JournalEntry, UpdateJournalEntry};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

pub mod markdown;

#[cfg(test)]
mod tests;

/// Columns + author join shared by every entry read. `user_id` surfaces as
/// `author_id`; `users.username` as `author_name`.
const SELECT_ENTRY: &str = "SELECT je.id, je.book_uuid, je.user_id AS author_id,
        u.username AS author_name, je.body_md, je.progress, je.status,
        je.created_at, je.updated_at
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
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, status)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(&input.body_md)
    .bind(input.progress.map(|p| p as i64))
    .bind(input.status.as_str())
    .fetch_one(pool)
    .await?;

    get_entry_by_id(pool, id).await
}

/// List a book's entries for `viewer_id`, newest first: every user's published
/// entries plus the viewer's own drafts (drafts stay per-user-private until
/// published). Returns an empty list — not an error — for an unknown uuid or a
/// book with no entries yet.
pub async fn list_journal_entries(
    pool: &SqlitePool,
    viewer_id: i64,
    book_uuid: &str,
) -> Result<Vec<JournalEntry>, JournalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(vec![]);
    };
    let rows = sqlx::query(&format!(
        "{SELECT_ENTRY} WHERE je.book_uuid = ?
            AND (je.status = 'published' OR je.user_id = ?)
          ORDER BY je.created_at DESC, je.id DESC LIMIT ?"
    ))
    .bind(&canonical)
    .bind(viewer_id)
    .bind(LIST_JOURNAL_ENTRIES_LIMIT)
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_entry).collect()
}

/// Edit an entry owned by `user_id`, returning the updated row. Errors with
/// `NotFound` when the id does not exist or belongs to another user (the two
/// cases are deliberately indistinguishable). Any journal image referenced in
/// the old body but not the new one is opportunistically GC'd — see
/// [`cleanup_orphaned_images`].
pub async fn update_journal_entry(
    pool: &SqlitePool,
    user_id: i64,
    entry_id: i64,
    input: &UpdateJournalEntry,
) -> Result<JournalEntry, JournalError> {
    let old_body: Option<String> =
        sqlx::query_scalar("SELECT body_md FROM journal_entries WHERE id = ? AND user_id = ?")
            .bind(entry_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let result = sqlx::query(
        "UPDATE journal_entries
            SET body_md = ?, progress = ?, status = COALESCE(?, status),
                updated_at = strftime('%s','now')
          WHERE id = ? AND user_id = ?",
    )
    .bind(&input.body_md)
    .bind(input.progress.map(|p| p as i64))
    .bind(input.status.map(|s| s.as_str()))
    .bind(entry_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(JournalError::NotFound);
    }
    if let Some(old_body) = old_body {
        let removed: HashSet<String> = crate::journal_images::referenced_image_names(&old_body)
            .difference(&crate::journal_images::referenced_image_names(
                &input.body_md,
            ))
            .cloned()
            .collect();
        cleanup_orphaned_images(pool, removed).await;
    }
    get_entry_by_id(pool, entry_id).await
}

/// Delete an entry owned by `user_id`. Errors with `NotFound` when the id does
/// not exist or belongs to another user. Any journal image uniquely
/// referenced by the deleted entry's body is opportunistically GC'd (e.g. the
/// composer's Cancel button discarding a draft) — see
/// [`cleanup_orphaned_images`].
pub async fn delete_journal_entry(
    pool: &SqlitePool,
    user_id: i64,
    entry_id: i64,
) -> Result<(), JournalError> {
    let deleted: Option<String> = sqlx::query_scalar(
        "DELETE FROM journal_entries WHERE id = ? AND user_id = ? RETURNING body_md",
    )
    .bind(entry_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(body_md) = deleted else {
        return Err(JournalError::NotFound);
    };
    cleanup_orphaned_images(
        pool,
        crate::journal_images::referenced_image_names(&body_md),
    )
    .await;
    Ok(())
}

/// Best-effort delete of any `candidates` no longer referenced by any
/// `journal_entries.body_md`. Called only after the caller's own mutation has
/// already committed, so no self-exclusion is needed: the caller's row either
/// no longer contains the name (edit) or no longer exists (delete). Failures —
/// a DB error on the reference check, or a panic in the filesystem unlink —
/// are logged and swallowed: this is opportunistic GC of an
/// orphanable-but-durable file store, not a correctness-critical path, and
/// must never fail the journal mutation that triggered it (mirrors
/// `set_settings`'s cover cleanup). [`purge::orphaned_images`] is the sibling
/// caller that instead propagates the DB error — see [`unreferenced_image_names`].
///
/// [`purge::orphaned_images`]: crate::deletion::purge::orphaned_images
async fn cleanup_orphaned_images(pool: &SqlitePool, candidates: HashSet<String>) {
    let orphaned = match unreferenced_image_names(pool, candidates).await {
        Ok(names) => names,
        Err(e) => {
            tracing::warn!(error = %e, "cleanup_orphaned_images: reference check failed");
            return;
        }
    };
    if orphaned.is_empty() {
        return;
    }
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        for name in &orphaned {
            crate::journal_images::delete_journal_image(name);
        }
    })
    .await
    {
        tracing::error!("cleanup_orphaned_images: spawn_blocking failed: {join_err}");
    }
}

/// Narrow `candidates` to the names no `journal_entries.body_md` still embeds
/// — matched on the full embed path rather than the bare name, since a
/// bare-name substring match could false-positive on an unrelated file whose
/// uuid happens to contain this one as a substring, or on stray body text.
///
/// Fetches every `body_md` once and matches all candidates in memory, rather
/// than one `instr()` scan per candidate — `journal_entries` is expected to
/// stay small enough that a single fetch always wins over N per-candidate
/// scans. Shared by [`cleanup_orphaned_images`] (swallow-and-log) and
/// `deletion::purge::orphaned_images` (propagate), which differ only in how
/// they handle the `Err` case — this helper stays agnostic and returns the
/// raw `sqlx::Error` for each caller to decide.
pub(crate) async fn unreferenced_image_names(
    pool: &SqlitePool,
    candidates: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, sqlx::Error> {
    let candidates: Vec<String> = candidates.into_iter().collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let bodies: Vec<String> = sqlx::query_scalar("SELECT body_md FROM journal_entries")
        .fetch_all(pool)
        .await?;
    Ok(candidates
        .into_iter()
        .filter(|name| {
            let embed = format!("{}{name}", markdown::IMAGE_URL_PREFIX);
            !bodies.iter().any(|body| body.contains(&embed))
        })
        .collect())
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
    let status: String = row.try_get("status")?;
    Ok(JournalEntry {
        id: row.try_get("id")?,
        book_uuid: row.try_get("book_uuid")?,
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        body_md,
        body_html,
        progress: progress.map(|p| p as u8),
        status: omnibus_shared::JournalStatus::from_db(&status),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
