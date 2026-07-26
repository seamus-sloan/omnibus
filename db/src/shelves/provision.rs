//! Provisioning the built-in per-user Wishlist shelf: one public, locked,
//! `wishlist_entries`-backed shelf per user. Migration `0047` seeds existing
//! users; [`provision_wishlist_shelves`] re-runs on boot and
//! [`provision_wishlist_shelf`] runs inline in `create_user` — both idempotent.

use sqlx::{Executor, Sqlite, SqlitePool};

use super::ShelfError;

/// Appended to the owner's username to form the system Wishlist shelf's
/// display name (`alice` → `alice's Wishlist`). The name is composed once at
/// provision time and never recomputed — usernames are immutable, and migration
/// `0054` renamed the rows seeded under the old shared `"Wishlist"` name.
pub(crate) const WISHLIST_NAME_SUFFIX: &str = "'s Wishlist";

/// Ensure one user has a Wishlist shelf. Idempotent *and* race-safe: the
/// `WHERE NOT EXISTS` guard skips the common already-provisioned case, and the
/// `INSERT OR IGNORE` lets the `idx_shelves_owner_wishlist` partial-unique index
/// absorb a concurrent double-provision (two writers both passing the guard)
/// without erroring. Runs against any executor so `create_user` can call it
/// inside its registration transaction — the `FROM users` join reads the row
/// that transaction just inserted, so the name is available before commit.
pub async fn provision_wishlist_shelf<'e, E>(exec: E, user_id: i64) -> Result<(), ShelfError>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT OR IGNORE INTO shelves (owner_user_id, kind, name, visibility)
         SELECT u.id, 'wishlist', u.username || ?2, 'public'
           FROM users u
          WHERE u.id = ?1
            AND NOT EXISTS (
              SELECT 1 FROM shelves WHERE owner_user_id = ?1 AND kind = 'wishlist'
          )",
    )
    .bind(user_id)
    .bind(WISHLIST_NAME_SUFFIX)
    .execute(exec)
    .await?;
    Ok(())
}

/// Boot backfill: provision a Wishlist shelf for every user missing one. A
/// no-op once caught up, so it runs on every `init_db`. `INSERT OR IGNORE` keeps
/// it race-safe against a concurrent `create_user` on a multi-instance boot.
pub async fn provision_wishlist_shelves(pool: &SqlitePool) -> Result<(), ShelfError> {
    sqlx::query(
        "INSERT OR IGNORE INTO shelves (owner_user_id, kind, name, visibility)
         SELECT id, 'wishlist', username || ?1, 'public' FROM users
          WHERE NOT EXISTS (
              SELECT 1 FROM shelves s WHERE s.owner_user_id = users.id AND s.kind = 'wishlist'
          )",
    )
    .bind(WISHLIST_NAME_SUFFIX)
    .execute(pool)
    .await?;
    Ok(())
}
