//! CRUD for the per-user physical wishlist. Orthogonal to digital ownership:
//! a user can wishlist a book whose EPUB they already have. Unique per
//! `(user_id, book_uuid)`, so re-wishlisting is idempotent.

use sqlx::SqlitePool;

use omnibus_shared::physical::{WishlistEntry, WishlistSource};

use super::PhysicalError;
use crate::books::resolve_canonical_book_uuid;

/// Hard cap on how many entries `list_wishlist` returns for a single user.
/// Practical usage stays well under this; the cap exists so a pathological
/// or automated wishlist pipeline can't produce an unbounded REST response,
/// mirroring `bookmarks::LIST_BOOKMARKS_LIMIT`.
pub const LIST_WISHLIST_LIMIT: i64 = 1_000;

/// A `wishlist_entries` row as read back from the DB, in column order.
type WishlistRow = (i64, i64, String, i64, String);

/// Map a DB row to a wire entry. The `source` column is CHECK-constrained to a
/// known value, so a parse miss can only mean corruption — fall back to
/// `Manual` rather than propagate an error the caller can't act on.
fn map_entry(r: WishlistRow) -> WishlistEntry {
    WishlistEntry {
        id: r.0,
        user_id: r.1,
        book_uuid: r.2,
        added_at: r.3,
        source: WishlistSource::from_db(&r.4).unwrap_or(WishlistSource::Manual),
    }
}

/// Add a book to a user's wishlist, returning the entry.
///
/// Folds the uuid to canonical `books.uuid` (honoring `merged_uuids`) and
/// stores that, so uniqueness holds across a merge and the row always resolves
/// to a live book; an unresolvable uuid returns [`PhysicalError::BookNotFound`].
/// Idempotent: a second add for the same `(user_id, canonical uuid)` returns the
/// existing row unchanged (the original `source`/`added_at` win).
pub async fn add_wishlist_entry(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    source: WishlistSource,
) -> Result<WishlistEntry, PhysicalError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(PhysicalError::BookNotFound)?;

    // ON CONFLICT ... DO UPDATE (a no-op self-assign) so RETURNING yields the
    // surviving row on both insert and conflict — a bare DO NOTHING returns no
    // row on conflict.
    let row = sqlx::query_as::<_, WishlistRow>(
        "INSERT INTO wishlist_entries (user_id, book_uuid, source)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET user_id = user_id
         RETURNING id, user_id, book_uuid, added_at, source",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(source.as_str())
    .fetch_one(pool)
    .await?;
    Ok(map_entry(row))
}

/// Remove a book from a user's wishlist. A no-op (not an error) when absent —
/// the desired end state (not on the wishlist) already holds.
///
/// Folds the uuid to canonical first so a `merged_uuids` ledger key still
/// deletes the row stored under the surviving book. An unresolvable uuid can
/// have no entry, so it's a no-op.
pub async fn remove_wishlist_entry(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<(), PhysicalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(());
    };
    sqlx::query("DELETE FROM wishlist_entries WHERE user_id = ?1 AND book_uuid = ?2")
        .bind(user_id)
        .bind(&canonical)
        .execute(pool)
        .await?;
    Ok(())
}

/// List a user's wishlist, newest first, capped at [`LIST_WISHLIST_LIMIT`].
pub async fn list_wishlist(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<WishlistEntry>, PhysicalError> {
    let rows = sqlx::query_as::<_, WishlistRow>(
        "SELECT id, user_id, book_uuid, added_at, source
           FROM wishlist_entries
          WHERE user_id = ?1
          ORDER BY added_at DESC, id DESC
          LIMIT ?2",
    )
    .bind(user_id)
    .bind(LIST_WISHLIST_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_entry).collect())
}
