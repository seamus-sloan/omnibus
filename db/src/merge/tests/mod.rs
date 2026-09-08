//! Unit tests for `merge_books` and `undo_merge`, split by sub-topic into
//! the sibling modules below; the user and uuid-lookup fixtures they share
//! live here.

mod forward_progress;
mod migration_0079;
mod reader_state;
mod relocation;
mod rescan;
mod undo;

async fn seed_user(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('admin', 'x', 1) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn book_id_by_uuid(pool: &sqlx::SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}
