//! Physical ownership data-layer tests, split to mirror the production
//! modules: physical copies, the per-user wishlist, fileless book creation
//! and removal, and the Physical pseudo-root promotion. Fixtures used by
//! more than one of those suites live here.

mod copies;
mod fileless;
mod promote;
mod remove;
mod wishlist;

use sqlx::SqlitePool;

async fn pool() -> SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    // Insert directly: `create_user` gates all but the first registration on the
    // registration-enabled setting, and these tests only need user rows for FKs.
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash) VALUES (?1, 'x') RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap()
}
