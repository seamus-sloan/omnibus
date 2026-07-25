//! CRUD for library-wide physical copies, plus wishlist fulfillment on
//! check-in. A copy is shared by all users (like a digital file); a book can
//! hold many, each individually deletable.

use omnibus_shared::physical::PhysicalCopy;
use sqlx::SqlitePool;

use super::PhysicalError;
use crate::books::{resolve_canonical_book_uuid, resolve_canonical_book_uuid_exec};

/// A `physical_copies` row as read back from the DB, in column order.
type CopyRow = (
    i64,
    String,
    Option<String>,
    Option<i64>,
    i64,
    Option<String>,
);

fn map_copy(r: CopyRow) -> PhysicalCopy {
    PhysicalCopy {
        id: r.0,
        book_uuid: r.1,
        isbn: r.2,
        added_by_user_id: r.3,
        checked_in_at: r.4,
        note: r.5,
    }
}

/// Check in a physical copy for a book, fulfilling every user's wishlist for it.
///
/// The uuid is folded to its canonical `books.uuid` (honoring `merged_uuids`)
/// before anything is written, so the stored copy and the wishlist sweep both
/// key on the same value the rest of the system uses; an unresolvable uuid
/// returns [`PhysicalError::BookNotFound`]. The insert and the sweep run in one
/// transaction, so a copy never lands without its fulfillment side effect.
pub async fn add_physical_copy(
    pool: &SqlitePool,
    book_uuid: &str,
    isbn: Option<&str>,
    added_by_user_id: Option<i64>,
    note: Option<&str>,
) -> Result<PhysicalCopy, PhysicalError> {
    let mut tx = pool.begin().await?;

    let canonical = resolve_canonical_book_uuid_exec(&mut *tx, book_uuid)
        .await?
        .ok_or(PhysicalError::BookNotFound)?;

    let row = sqlx::query_as::<_, CopyRow>(
        "INSERT INTO physical_copies (book_uuid, isbn, added_by_user_id, note)
         VALUES (?1, ?2, ?3, ?4)
         RETURNING id, book_uuid, isbn, added_by_user_id, checked_in_at, note",
    )
    .bind(&canonical)
    .bind(isbn)
    .bind(added_by_user_id)
    .bind(note)
    .fetch_one(&mut *tx)
    .await?;

    // Fulfillment: a checked-in copy clears the book from EVERY user's wishlist.
    sqlx::query("DELETE FROM wishlist_entries WHERE book_uuid = ?1")
        .bind(&canonical)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(map_copy(row))
}

/// List a book's physical copies, oldest check-in first.
///
/// Folds the uuid to canonical first, so copies are found when the caller
/// passes a `merged_uuids` ledger key. An unresolvable uuid has no copies.
pub async fn list_physical_copies(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<PhysicalCopy>, PhysicalError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query_as::<_, CopyRow>(
        "SELECT id, book_uuid, isbn, added_by_user_id, checked_in_at, note
           FROM physical_copies
          WHERE book_uuid = ?1
          ORDER BY checked_in_at, id",
    )
    .bind(&canonical)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_copy).collect())
}

/// Replace a copy's free-text note, returning the updated row. `None` clears
/// it. Returns [`PhysicalError::CopyNotFound`] if no copy has that id.
pub async fn update_physical_copy_note(
    pool: &SqlitePool,
    copy_id: i64,
    note: Option<&str>,
) -> Result<PhysicalCopy, PhysicalError> {
    // Treat a blank note as a clear, so the UI's empty input doesn't persist an
    // empty string that renders as a stray blank line on the copy card.
    let note = note.map(str::trim).filter(|s| !s.is_empty());
    let row = sqlx::query_as::<_, CopyRow>(
        "UPDATE physical_copies SET note = ?2 WHERE id = ?1
         RETURNING id, book_uuid, isbn, added_by_user_id, checked_in_at, note",
    )
    .bind(copy_id)
    .bind(note)
    .fetch_optional(pool)
    .await?
    .ok_or(PhysicalError::CopyNotFound)?;
    Ok(map_copy(row))
}

/// Delete a single physical copy ("I sold it"). Returns
/// [`PhysicalError::CopyNotFound`] if no copy has that id.
pub async fn delete_physical_copy(pool: &SqlitePool, copy_id: i64) -> Result<(), PhysicalError> {
    let res = sqlx::query("DELETE FROM physical_copies WHERE id = ?1")
        .bind(copy_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(PhysicalError::CopyNotFound);
    }
    Ok(())
}
