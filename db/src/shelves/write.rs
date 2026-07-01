//! Shelf mutations: create, update, delete, and hand-picked membership edits.
//!
//! Hand-picked uuids are resolved to the canonical `books.uuid`
//! ([`resolve_canonical_book_uuid`]) before storage so a format-merged input
//! still points at the surviving book. Name collisions per owner surface as
//! [`ShelfError::NameTaken`].

use sqlx::SqlitePool;

use omnibus_shared::{CreateShelfRequest, Shelf, ShelfKind, UpdateShelfRequest};

use super::read::get_shelf;
use super::ShelfError;
use crate::resolve_canonical_book_uuid;

/// Accent swatches assigned round-robin by the owner's existing shelf count.
const ACCENTS: [&str; 6] = [
    "#c9a15a", "#7fa7c9", "#c98b8b", "#8bc9a1", "#b08bc9", "#c9b487",
];

/// Create a shelf owned by `owner_id` and return its full detail. Manual book
/// uuids are resolved up front so a bad uuid fails before the shelf row exists.
pub async fn create_shelf(
    pool: &SqlitePool,
    owner_id: i64,
    req: &CreateShelfRequest,
) -> Result<Shelf, ShelfError> {
    let resolved = resolve_all(pool, &req.book_uuids).await?;
    let accent = pick_accent(pool, owner_id).await?;
    let position = next_shelf_position(pool, owner_id).await?;

    let id: i64 = sqlx::query_scalar::<_, i64>(
        "INSERT INTO shelves
            (owner_user_id, kind, name, description, visibility, match_mode, accent, position)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(owner_id)
    .bind(req.kind.as_str())
    .bind(req.name.trim())
    .bind(req.description.as_deref())
    .bind(req.visibility.as_str())
    .bind(req.match_mode.map(|m| m.as_str()))
    .bind(&accent)
    .bind(position)
    .fetch_one(pool)
    .await
    .map_err(map_unique)?;

    if req.kind == ShelfKind::Smart {
        replace_rules(pool, id, req).await?;
    }
    insert_books(pool, id, &resolved, owner_id, 0).await?;

    get_shelf(pool, id).await?.ok_or(ShelfError::NotFound)
}

/// Apply a partial update. `None` fields are untouched; `rules` (when present)
/// replaces the whole rule set. Returns the updated shelf.
pub async fn update_shelf(
    pool: &SqlitePool,
    id: i64,
    req: &UpdateShelfRequest,
) -> Result<Shelf, ShelfError> {
    if get_shelf(pool, id).await?.is_none() {
        return Err(ShelfError::NotFound);
    }
    sqlx::query(
        "UPDATE shelves SET
            name        = COALESCE(?, name),
            description = COALESCE(?, description),
            visibility  = COALESCE(?, visibility),
            match_mode  = COALESCE(?, match_mode),
            updated_at  = strftime('%s','now')
         WHERE id = ?",
    )
    .bind(req.name.as_deref().map(str::trim))
    .bind(req.description.as_deref())
    .bind(req.visibility.map(|v| v.as_str()))
    .bind(req.match_mode.map(|m| m.as_str()))
    .bind(id)
    .execute(pool)
    .await
    .map_err(map_unique)?;

    if let Some(rules) = &req.rules {
        sqlx::query("DELETE FROM shelf_rules WHERE shelf_id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        for (i, rule) in rules.iter().enumerate() {
            insert_rule(pool, id, rule, i as i64).await?;
        }
    }

    get_shelf(pool, id).await?.ok_or(ShelfError::NotFound)
}

/// Delete a shelf (cascades its rules + membership). `NotFound` if absent.
pub async fn delete_shelf(pool: &SqlitePool, id: i64) -> Result<(), ShelfError> {
    let res = sqlx::query("DELETE FROM shelves WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ShelfError::NotFound);
    }
    Ok(())
}

/// Append books to a hand-picked shelf. Each uuid is resolved to canonical
/// first; an unknown uuid fails the whole call. Already-present books are
/// silently kept (INSERT OR IGNORE).
pub async fn add_books(
    pool: &SqlitePool,
    shelf_id: i64,
    uuids: &[String],
    added_by: i64,
) -> Result<(), ShelfError> {
    let resolved = resolve_all(pool, uuids).await?;
    let start = next_book_position(pool, shelf_id).await?;
    insert_books(pool, shelf_id, &resolved, added_by, start).await
}

/// Remove a book from a hand-picked shelf. A no-op when it isn't on the shelf.
pub async fn remove_book(pool: &SqlitePool, shelf_id: i64, uuid: &str) -> Result<(), ShelfError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, uuid).await? else {
        return Ok(());
    };
    sqlx::query("DELETE FROM shelf_books WHERE shelf_id = ? AND book_uuid = ?")
        .bind(shelf_id)
        .bind(canonical)
        .execute(pool)
        .await?;
    Ok(())
}

// --- helpers ---------------------------------------------------------------

/// Resolve every uuid to canonical form; error on the first unknown one.
async fn resolve_all(pool: &SqlitePool, uuids: &[String]) -> Result<Vec<String>, ShelfError> {
    let mut out = Vec::with_capacity(uuids.len());
    for uuid in uuids {
        out.push(
            resolve_canonical_book_uuid(pool, uuid)
                .await?
                .ok_or(ShelfError::BookNotFound)?,
        );
    }
    Ok(out)
}

async fn insert_books(
    pool: &SqlitePool,
    shelf_id: i64,
    uuids: &[String],
    added_by: i64,
    start_pos: i64,
) -> Result<(), ShelfError> {
    for (i, uuid) in uuids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO shelf_books (shelf_id, book_uuid, position, added_by_user_id)
             VALUES (?, ?, ?, ?)",
        )
        .bind(shelf_id)
        .bind(uuid)
        .bind(start_pos + i as i64)
        .bind(added_by)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn replace_rules(
    pool: &SqlitePool,
    shelf_id: i64,
    req: &CreateShelfRequest,
) -> Result<(), ShelfError> {
    for (i, rule) in req.rules.iter().enumerate() {
        insert_rule(pool, shelf_id, rule, i as i64).await?;
    }
    Ok(())
}

async fn insert_rule(
    pool: &SqlitePool,
    shelf_id: i64,
    rule: &omnibus_shared::ShelfRule,
    position: i64,
) -> Result<(), ShelfError> {
    sqlx::query(
        "INSERT INTO shelf_rules (shelf_id, field, op, value, position) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(shelf_id)
    .bind(rule.field.as_str())
    .bind(rule.op.as_str())
    .bind(&rule.value)
    .bind(position)
    .execute(pool)
    .await?;
    Ok(())
}

async fn pick_accent(pool: &SqlitePool, owner_id: i64) -> Result<String, ShelfError> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shelves WHERE owner_user_id = ?")
        .bind(owner_id)
        .fetch_one(pool)
        .await?;
    Ok(ACCENTS[(n as usize) % ACCENTS.len()].to_string())
}

async fn next_shelf_position(pool: &SqlitePool, owner_id: i64) -> Result<i64, ShelfError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM shelves WHERE owner_user_id = ?",
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await?)
}

async fn next_book_position(pool: &SqlitePool, shelf_id: i64) -> Result<i64, ShelfError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM shelf_books WHERE shelf_id = ?",
    )
    .bind(shelf_id)
    .fetch_one(pool)
    .await?)
}

/// Map a UNIQUE(owner, name) violation to [`ShelfError::NameTaken`]; pass every
/// other DB error through.
fn map_unique(e: sqlx::Error) -> ShelfError {
    if let sqlx::Error::Database(db) = &e {
        if db.is_unique_violation() {
            return ShelfError::NameTaken;
        }
    }
    ShelfError::Sqlx(e)
}
