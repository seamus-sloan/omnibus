//! Shelf mutations: create, update, delete, and hand-picked membership edits.
//!
//! Hand-picked uuids are resolved to the canonical `books.uuid`
//! ([`resolve_canonical_book_uuid`]) before storage so a format-merged input
//! still points at the surviving book. Name collisions per owner surface as
//! [`ShelfError::NameTaken`].

use omnibus_shared::{
    CreateShelfRequest, MatchMode, Shelf, ShelfKind, ShelfRule, UpdateShelfRequest, Visibility,
};
use sqlx::{Sqlite, SqlitePool, Transaction};

use super::read::get_shelf;
use super::ShelfError;
use crate::{resolve_canonical_book_uuid, resolve_canonical_book_uuids_bulk_exec};

/// Accent swatches assigned round-robin by the owner's existing shelf count.
const ACCENTS: [&str; 6] = [
    "#c9a15a", "#7fa7c9", "#c98b8b", "#8bc9a1", "#b08bc9", "#c9b487",
];

/// Create a shelf owned by `owner_id` and return its full detail. Manual book
/// uuids are resolved up front so a bad uuid fails before the shelf row exists.
/// The shelf row, its rule set, and its membership all land in one
/// transaction so a mid-write failure can't leave a shelf with a partial rule
/// set or partial membership.
pub async fn create_shelf(
    pool: &SqlitePool,
    owner_id: i64,
    req: &CreateShelfRequest,
) -> Result<Shelf, ShelfError> {
    let resolved = resolve_all(pool, &req.book_uuids).await?;
    let accent = pick_accent(pool, owner_id).await?;
    let position = next_shelf_position(pool, owner_id).await?;

    let mut tx = pool.begin().await?;

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
    .bind(req.match_mode.map(MatchMode::as_str))
    .bind(&accent)
    .bind(position)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_unique)?;

    if req.kind == ShelfKind::Smart {
        insert_rules(&mut tx, id, &req.rules).await?;
    }
    insert_books(&mut tx, id, &resolved, owner_id, 0).await?;

    tx.commit().await?;

    get_shelf(pool, id).await?.ok_or(ShelfError::NotFound)
}

/// Apply a partial update. `None` fields are untouched; `rules` (when present)
/// replaces the whole rule set. Returns the updated shelf.
///
/// The rule replacement runs *before* the shelf-row `UPDATE` within the same
/// transaction: both share one commit, so a rename that collides with an
/// existing name (`ShelfError::NameTaken`) rolls the rule replacement back
/// too, instead of leaving the new rules committed under the old name.
pub async fn update_shelf(
    pool: &SqlitePool,
    id: i64,
    req: &UpdateShelfRequest,
) -> Result<Shelf, ShelfError> {
    let Some(existing) = get_shelf(pool, id).await? else {
        return Err(ShelfError::NotFound);
    };
    // A system shelf (Wishlist) is locked: no rename, description, visibility,
    // or rule edits. Rejected here so both web and mobile mutation paths hit it.
    if existing.kind.is_system() {
        return Err(ShelfError::SystemShelf);
    }
    // Keep the kind invariant: a manual shelf can't take a match mode or rules,
    // and a smart shelf can't be emptied to zero conditions. Without this an
    // update could leave a manual shelf reporting `match_mode: Some(...)`.
    match existing.kind {
        ShelfKind::Manual if req.match_mode.is_some() || req.rules.is_some() => {
            return Err(ShelfError::InvalidRule(
                "manual shelves cannot have a match mode or rules".into(),
            ));
        }
        ShelfKind::Smart if matches!(&req.rules, Some(r) if r.is_empty()) => {
            return Err(ShelfError::InvalidRule(
                "a smart shelf needs at least one condition".into(),
            ));
        }
        _ => {}
    }

    let mut tx = pool.begin().await?;

    if let Some(rules) = &req.rules {
        sqlx::query("DELETE FROM shelf_rules WHERE shelf_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        insert_rules(&mut tx, id, rules).await?;
    }

    sqlx::query(
        "UPDATE shelves SET
            name         = COALESCE(?, name),
            description  = COALESCE(?, description),
            visibility   = COALESCE(?, visibility),
            match_mode   = COALESCE(?, match_mode),
            sync_to_kobo = COALESCE(?, sync_to_kobo),
            updated_at   = strftime('%s','now')
         WHERE id = ?",
    )
    .bind(req.name.as_deref().map(str::trim))
    .bind(req.description.as_deref())
    .bind(req.visibility.map(Visibility::as_str))
    .bind(req.match_mode.map(MatchMode::as_str))
    .bind(req.sync_to_kobo.map(i64::from))
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(map_unique)?;

    tx.commit().await?;

    get_shelf(pool, id).await?.ok_or(ShelfError::NotFound)
}

/// Delete a shelf (cascades its rules + membership). `NotFound` if absent,
/// `SystemShelf` if it's the built-in Wishlist. The kind check reads the row
/// first so a delete can't slip past the guard a raw `DELETE` would miss.
pub async fn delete_shelf(pool: &SqlitePool, id: i64) -> Result<(), ShelfError> {
    match get_shelf(pool, id).await? {
        None => return Err(ShelfError::NotFound),
        Some(s) if s.kind.is_system() => return Err(ShelfError::SystemShelf),
        Some(_) => {}
    }
    let res = sqlx::query("DELETE FROM shelves WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ShelfError::NotFound);
    }
    Ok(())
}

/// Reject a hand-picked membership edit against a system shelf, whose members
/// are derived (Wishlist ← `wishlist_entries`). `NotFound` if the shelf is
/// absent. Shared by [`add_books`] and [`remove_book`].
async fn reject_system_membership_edit(pool: &SqlitePool, shelf_id: i64) -> Result<(), ShelfError> {
    match get_shelf(pool, shelf_id).await? {
        None => Err(ShelfError::NotFound),
        Some(s) if s.kind.is_system() => Err(ShelfError::SystemShelf),
        Some(_) => Ok(()),
    }
}

/// Append books to a hand-picked shelf. Each uuid is resolved to canonical
/// first; an unknown uuid fails the whole call. Already-present books are
/// silently kept (INSERT OR IGNORE). The batch insert itself lands in one
/// transaction, so a mid-batch failure can't leave a partial set of rows.
pub async fn add_books(
    pool: &SqlitePool,
    shelf_id: i64,
    uuids: &[String],
    added_by: i64,
) -> Result<(), ShelfError> {
    reject_system_membership_edit(pool, shelf_id).await?;
    let resolved = resolve_all(pool, uuids).await?;
    let start = next_book_position(pool, shelf_id).await?;

    let mut tx = pool.begin().await?;
    insert_books(&mut tx, shelf_id, &resolved, added_by, start).await?;
    tx.commit().await?;
    Ok(())
}

/// Remove a book from a hand-picked shelf. A no-op when it isn't on the shelf.
pub async fn remove_book(pool: &SqlitePool, shelf_id: i64, uuid: &str) -> Result<(), ShelfError> {
    reject_system_membership_edit(pool, shelf_id).await?;
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

/// Resolve every uuid to canonical form in one batched round-trip (chunked
/// `IN (...)`, see [`resolve_canonical_book_uuids_bulk_exec`]) instead of one
/// query per uuid; errors on the first unknown one, preserving input order.
async fn resolve_all(pool: &SqlitePool, uuids: &[String]) -> Result<Vec<String>, ShelfError> {
    let mut tx = pool.begin().await?;
    let resolved = resolve_canonical_book_uuids_bulk_exec(&mut tx, uuids).await?;
    tx.commit().await?;
    uuids
        .iter()
        .map(|uuid| resolved.get(uuid).cloned().ok_or(ShelfError::BookNotFound))
        .collect()
}

/// Batch-insert `shelf_books` rows in a single transaction. Chunked at 200
/// rows (4 binds/row keeps each statement under SQLite's 999 bind-parameter
/// cap) instead of one round-trip per book.
async fn insert_books(
    tx: &mut Transaction<'_, Sqlite>,
    shelf_id: i64,
    uuids: &[String],
    added_by: i64,
    start_pos: i64,
) -> Result<(), ShelfError> {
    const CHUNK_SIZE: usize = 200;
    for (chunk_idx, chunk) in uuids.chunks(CHUNK_SIZE).enumerate() {
        let rows = std::iter::repeat_n("(?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO shelf_books (shelf_id, book_uuid, position, added_by_user_id)
             VALUES {rows}"
        );
        let base = start_pos + (chunk_idx * CHUNK_SIZE) as i64;
        let mut q = sqlx::query(&sql);
        for (i, uuid) in chunk.iter().enumerate() {
            q = q
                .bind(shelf_id)
                .bind(uuid)
                .bind(base + i as i64)
                .bind(added_by);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Batch-insert `shelf_rules` rows in rule order, in a single transaction.
/// Chunked at 199 rows (5 binds/row keeps each statement under SQLite's 999
/// bind-parameter cap) instead of one round-trip per rule.
async fn insert_rules(
    tx: &mut Transaction<'_, Sqlite>,
    shelf_id: i64,
    rules: &[ShelfRule],
) -> Result<(), ShelfError> {
    const CHUNK_SIZE: usize = 199;
    for (chunk_idx, chunk) in rules.chunks(CHUNK_SIZE).enumerate() {
        let rows = std::iter::repeat_n("(?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("INSERT INTO shelf_rules (shelf_id, field, op, value, position) VALUES {rows}");
        let base = (chunk_idx * CHUNK_SIZE) as i64;
        let mut q = sqlx::query(&sql);
        for (i, rule) in chunk.iter().enumerate() {
            q = q
                .bind(shelf_id)
                .bind(rule.field.as_str())
                .bind(rule.op.as_str())
                .bind(&rule.value)
                .bind(base + i as i64);
        }
        q.execute(&mut **tx).await?;
    }
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
