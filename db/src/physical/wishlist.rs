//! CRUD for the per-user physical wishlist. Orthogonal to digital ownership:
//! a user can wishlist a book whose EPUB they already have. Unique per
//! `(user_id, book_uuid)`, so re-wishlisting is idempotent.

use omnibus_shared::physical::{WishlistEntry, WishlistRemoval, WishlistSource};
use sqlx::SqlitePool;

use super::{PhysicalError, PHYSICAL_LIBRARY_PATH};
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
///
/// A book that existed only to be wanted goes with the last entry that wanted
/// it: one minted under the physical pseudo-root by a wishlist add, with no
/// file, no checked-in copy, and no other reader's entry left. Kept, it is a
/// row no listing shows and no shelf holds, reachable only by its URL and
/// still announcing itself there as tracked from a collection or wishlist it
/// is on neither of. The purge is the same sweep as
/// [`super::delete_fileless_book`], so whatever a reader wrote against the
/// orphan through that page goes with it. A ghosted library book — file gone,
/// root real — is never touched here: the reindex owns that row and its
/// retention.
pub async fn remove_wishlist_entry(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<WishlistRemoval, PhysicalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(WishlistRemoval::default());
    };
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM wishlist_entries WHERE user_id = ?1 AND book_uuid = ?2")
        .bind(user_id)
        .bind(&canonical)
        .execute(&mut *tx)
        .await?;

    let orphan: Option<i64> = sqlx::query_scalar(
        "SELECT b.id FROM books b
           JOIN scan_roots l ON l.id = b.library_id
          WHERE b.uuid = ?1
            AND l.path = ?2
            AND NOT EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
            AND NOT EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM wishlist_entries we WHERE we.book_uuid = b.uuid)",
    )
    .bind(&canonical)
    .bind(PHYSICAL_LIBRARY_PATH)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(book_id) = orphan else {
        tx.commit().await?;
        return Ok(WishlistRemoval::default());
    };

    crate::deletion::purge::purge_book(&mut tx, book_id, &canonical)
        .await
        .map_err(super::remove::map_delete_error)?;
    tx.commit().await?;

    super::remove::discard_cover_files(canonical).await;
    Ok(WishlistRemoval { book_deleted: true })
}

/// Fetch one user's wishlist entry for a book, or `None` when it isn't
/// wishlisted. Folds the uuid to canonical first, matching [`add_wishlist_entry`]
/// — book detail calls this to decide between the "Add to physical wishlist"
/// action and the tracking card.
pub async fn get_wishlist_entry(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<WishlistEntry>, PhysicalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let row = sqlx::query_as::<_, WishlistRow>(
        "SELECT id, user_id, book_uuid, added_at, source
           FROM wishlist_entries
          WHERE user_id = ?1 AND book_uuid = ?2",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_entry))
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
