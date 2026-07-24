//! Provisioning the built-in per-user Wishlist shelf.
//!
//! Every user has exactly one `kind='wishlist'` shelf: public, locked, and
//! backed by `wishlist_entries`. Migration `0047` seeds existing users at
//! migrate time; [`provision_wishlist_shelves`] re-runs idempotently on every
//! boot (catching any user the migration missed), and [`provision_wishlist_shelf`]
//! provisions a single new registration inline in `create_user`.

use sqlx::{Executor, Sqlite, SqlitePool};

use super::ShelfError;

/// The immutable display name of the system Wishlist shelf.
pub(crate) const WISHLIST_SHELF_NAME: &str = "Wishlist";

/// Ensure one user has a Wishlist shelf. Idempotent: the `WHERE NOT EXISTS`
/// guard plus the `idx_shelves_owner_wishlist` partial-unique index make a
/// second call a no-op. Runs against any executor so `create_user` can call it
/// inside its registration transaction.
pub async fn provision_wishlist_shelf<'e, E>(exec: E, user_id: i64) -> Result<(), ShelfError>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO shelves (owner_user_id, kind, name, visibility)
         SELECT ?1, 'wishlist', ?2, 'public'
          WHERE NOT EXISTS (
              SELECT 1 FROM shelves WHERE owner_user_id = ?1 AND kind = 'wishlist'
          )",
    )
    .bind(user_id)
    .bind(WISHLIST_SHELF_NAME)
    .execute(exec)
    .await?;
    Ok(())
}

/// Boot backfill: provision a Wishlist shelf for every user missing one. A
/// no-op once caught up, so it runs on every `init_db`.
pub async fn provision_wishlist_shelves(pool: &SqlitePool) -> Result<(), ShelfError> {
    sqlx::query(
        "INSERT INTO shelves (owner_user_id, kind, name, visibility)
         SELECT id, 'wishlist', ?1, 'public' FROM users
          WHERE NOT EXISTS (
              SELECT 1 FROM shelves s WHERE s.owner_user_id = users.id AND s.kind = 'wishlist'
          )",
    )
    .bind(WISHLIST_SHELF_NAME)
    .execute(pool)
    .await?;
    Ok(())
}
