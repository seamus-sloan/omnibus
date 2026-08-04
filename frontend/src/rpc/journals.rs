//! Public journal-entry CRUD plus the composer's live markdown preview.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::{CreateJournalEntry, JournalEntry, UpdateJournalEntry};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Create a public journal entry on a book. Mobile uses the analogous REST
/// route in `server::backend::journals`; the rest of this family follows the
/// same web-vs-mobile split.
#[post("/api/rpc/journals/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_journal_entry(input: CreateJournalEntry) -> Result<JournalEntry> {
    if let Err(msg) = input.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::journals::create_journal_entry(&pool.0, user.id, &input).await {
        Ok(entry) => Ok(entry),
        Err(db::journals::JournalError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::journals::JournalError::NotFound) => {
            Err(ServerFnError::new("journal entry not found").into())
        }
        Err(db::journals::JournalError::Sqlx(e)) => {
            Err(internal_rpc_error("create journal entry", e).into())
        }
    }
}

/// List a book's journal entries, newest first: every user's published entries
/// (journals are public) plus the caller's own drafts. Returns an empty list
/// when the uuid is unknown or has no entries yet.
#[post("/api/rpc/journals/list", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_journal_entries(book_uuid: String) -> Result<Vec<JournalEntry>> {
    match db::journals::list_journal_entries(&pool.0, user.id, &book_uuid).await {
        Ok(list) => Ok(list),
        Err(e) => Err(internal_rpc_error("list journal entries", e).into()),
    }
}

/// Edit a journal entry owned by the current user. Errors with
/// `"journal entry not found"` when the id does not exist or belongs to another
/// user (the two cases are deliberately indistinguishable).
#[post("/api/rpc/journals/update", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_journal_entry(id: i64, input: UpdateJournalEntry) -> Result<JournalEntry> {
    if let Err(msg) = input.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::journals::update_journal_entry(&pool.0, user.id, id, &input).await {
        Ok(entry) => Ok(entry),
        Err(db::journals::JournalError::NotFound) => {
            Err(ServerFnError::new("journal entry not found").into())
        }
        Err(e) => Err(internal_rpc_error("update journal entry", e).into()),
    }
}

/// Delete a journal entry owned by the current user. Errors with
/// `"journal entry not found"` when the id does not exist or belongs to another
/// user.
#[post("/api/rpc/journals/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_journal_entry(id: i64) -> Result<()> {
    match db::journals::delete_journal_entry(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(db::journals::JournalError::NotFound) => {
            Err(ServerFnError::new("journal entry not found").into())
        }
        Err(e) => Err(internal_rpc_error("delete journal entry", e).into()),
    }
}

/// Render a draft journal body to sanitized HTML for the composer's live
/// preview, using the same renderer as the persisted read path. Rejects bodies
/// over `BODY_MAX_LEN` so an authenticated client can't drive arbitrary
/// markdown+sanitize work.
#[post("/api/rpc/journals/preview", _pool: PoolExt, _user: AuthUser)]
pub async fn rpc_preview_journal_markdown(body_md: String) -> Result<String> {
    if body_md.len() > omnibus_shared::BODY_MAX_LEN {
        return Err(ServerFnError::new(format!(
            "journal entry must be {} bytes or fewer",
            omnibus_shared::BODY_MAX_LEN
        ))
        .into());
    }
    Ok(db::journals::markdown::render(&body_md))
}
