//! CRUD for library-wide physical copies, plus wishlist fulfillment on
//! check-in. A copy is shared by all users (like a digital file); a book can
//! hold many, each individually deletable.

use sqlx::SqlitePool;

use omnibus_shared::physical::PhysicalCopy;

use super::PhysicalError;
use crate::books::resolve_book_id_by_uuid_exec;

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
/// The book must resolve (honoring `merged_uuids`) or this returns
/// [`PhysicalError::BookNotFound`]. The insert and the wishlist sweep run in one
/// transaction, so a copy never lands without its fulfillment side effect.
pub async fn add_physical_copy(
    pool: &SqlitePool,
    book_uuid: &str,
    isbn: Option<&str>,
    added_by_user_id: Option<i64>,
    note: Option<&str>,
) -> Result<PhysicalCopy, PhysicalError> {
    let mut tx = pool.begin().await?;

    if resolve_book_id_by_uuid_exec(&mut *tx, book_uuid)
        .await?
        .is_none()
    {
        return Err(PhysicalError::BookNotFound);
    }

    let row = sqlx::query_as::<_, CopyRow>(
        "INSERT INTO physical_copies (book_uuid, isbn, added_by_user_id, note)
         VALUES (?1, ?2, ?3, ?4)
         RETURNING id, book_uuid, isbn, added_by_user_id, checked_in_at, note",
    )
    .bind(book_uuid)
    .bind(isbn)
    .bind(added_by_user_id)
    .bind(note)
    .fetch_one(&mut *tx)
    .await?;

    // Fulfillment: a checked-in copy clears the book from EVERY user's wishlist.
    sqlx::query("DELETE FROM wishlist_entries WHERE book_uuid = ?1")
        .bind(book_uuid)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(map_copy(row))
}

/// List a book's physical copies, oldest check-in first.
pub async fn list_physical_copies(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<PhysicalCopy>, PhysicalError> {
    let rows = sqlx::query_as::<_, CopyRow>(
        "SELECT id, book_uuid, isbn, added_by_user_id, checked_in_at, note
           FROM physical_copies
          WHERE book_uuid = ?1
          ORDER BY checked_in_at, id",
    )
    .bind(book_uuid)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_copy).collect())
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
