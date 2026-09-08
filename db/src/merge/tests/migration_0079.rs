//! Migration 0079's one-time heal of reader rows stranded on a merged-away
//! uuid: migrate to 0078, seed the orphan state, then apply 0079 alone and
//! assert the repoint, the newer-row tie-breaks, the no-op on a database
//! that never merged, and colliding orphans.

use crate::test_support::count_rows as count;

/// Version of `0079_merge_orphan_user_data.sql`.
const MERGE_ORPHAN_VERSION: i64 = 79;

/// A pool migrated to just *below* [`MERGE_ORPHAN_VERSION`] — the schema an
/// install carrying merge-stranded rows sits at before the heal lands. One
/// connection, so the in-memory database is a single shared one.
async fn pool_before_merge_orphan_heal() -> sqlx::SqlitePool {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for m in MIGRATOR.iter().filter(|m| m.version < MERGE_ORPHAN_VERSION) {
        sqlx::raw_sql(&m.sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn apply_merge_orphan_migration(pool: &sqlx::SqlitePool) {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    let m = MIGRATOR
        .iter()
        .find(|m| m.version == MERGE_ORPHAN_VERSION)
        .expect("0079 must exist");
    sqlx::raw_sql(&m.sql).execute(pool).await.unwrap();
}

/// The state an old merge left behind: a surviving book, the merged-away uuid
/// recorded in the attach ledger, and no `books` row carrying it.
async fn seed_merge_orphan_state(pool: &sqlx::SqlitePool) {
    sqlx::raw_sql(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'x', 1);
         INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'uuid-live', 1, '/lib/a.epub', 'Dracula');
         INSERT INTO merged_uuids (uuid, book_id, format, library_path)
              VALUES ('uuid-gone', 1, 'M4B', '/audio');",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn migration_0079_repoints_user_data_stranded_by_an_earlier_merge() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    // The reader's record sits entirely on the merged-away uuid, where the
    // book page cannot see it and the trailing-12 chart still counts it.
    sqlx::raw_sql(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
              VALUES (1, 'uuid-gone', 'finished', 2000, 2000);
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-gone', 9, 2000);
         INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at, updated_at)
              VALUES (1, 'uuid-gone', 'finished it', 100, 2000, 2000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    for table in ["book_read_status", "user_ratings", "journal_entries"] {
        let on_live = count(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE book_uuid = 'uuid-live'"),
        )
        .await;
        let stranded = count(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE book_uuid = 'uuid-gone'"),
        )
        .await;
        assert_eq!(on_live, 1, "{table} must follow the surviving book");
        assert_eq!(stranded, 0, "{table} must leave nothing on the dead uuid");
    }
}

#[tokio::test]
async fn migration_0079_keeps_the_newer_row_when_both_sides_carry_one() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    // Both sides rated and finished — the stranded side is the newer one, so
    // latest-wins must replace the surviving book's row rather than collide
    // against UNIQUE (user_id, book_uuid).
    sqlx::raw_sql(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
              VALUES (1, 'uuid-live', 'reading', 1000, NULL),
                     (1, 'uuid-gone', 'finished', 2000, 2000);
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-live', 4, 1000),
                     (1, 'uuid-gone', 9, 2000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![("uuid-live".to_string(), 9)]);

    let status: Vec<(String, String)> =
        sqlx::query_as("SELECT book_uuid, status FROM book_read_status")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        status,
        vec![("uuid-live".to_string(), "finished".to_string())]
    );
}

#[tokio::test]
async fn migration_0079_keeps_the_surviving_books_row_when_it_is_newer() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    sqlx::raw_sql(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-live', 4, 3000),
                     (1, 'uuid-gone', 9, 2000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![("uuid-live".to_string(), 4)]);
}

#[tokio::test]
async fn migration_0079_is_a_no_op_on_a_database_that_never_merged() {
    let pool = pool_before_merge_orphan_heal().await;
    sqlx::raw_sql(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'x', 1);
         INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'uuid-live', 1, '/lib/a.epub', 'Dracula');
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-live', 7, 1000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![("uuid-live".to_string(), 7)]);
}

#[tokio::test]
async fn migration_0079_resolves_two_orphans_landing_on_the_same_book() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    // A chain of merges (A into B, then B into C) leaves several orphan uuids
    // pointing at one live book, so a reader can hold rows on two of them.
    // Repointing both without resolving them first violates
    // UNIQUE (user_id, book_uuid) and aborts the upgrade at startup.
    sqlx::raw_sql(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
              VALUES ('uuid-gone-2', 1, 'EPUB', '/books');
         INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
              VALUES (1, 'uuid-gone', 'finished', 1000, 1000),
                     (1, 'uuid-gone-2', 'reading', 2000, NULL);
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-gone', 4, 1000),
                     (1, 'uuid-gone-2', 9, 2000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    // One row each, on the survivor, carrying the newer orphan's values.
    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![("uuid-live".to_string(), 9)]);

    let status: Vec<(String, String)> =
        sqlx::query_as("SELECT book_uuid, status FROM book_read_status")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        status,
        vec![("uuid-live".to_string(), "reading".to_string())]
    );
}

#[tokio::test]
async fn migration_0079_resolves_two_orphans_against_a_live_row_too() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    // Three-way: two orphans plus a row already on the survivor. The newest
    // wins across the whole group, not just pairwise.
    sqlx::raw_sql(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
              VALUES ('uuid-gone-2', 1, 'EPUB', '/books');
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-gone', 2, 1000),
                     (1, 'uuid-gone-2', 6, 3000),
                     (1, 'uuid-live', 4, 2000);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![("uuid-live".to_string(), 6)]);
}

#[tokio::test]
async fn migration_0079_keeps_each_readers_own_row_when_orphans_collide() {
    let pool = pool_before_merge_orphan_heal().await;
    seed_merge_orphan_state(&pool).await;
    // The dedupe groups by (user, surviving book) — one reader's rows must
    // never delete another's.
    sqlx::raw_sql(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (2, 'v', 'x', 0);
         INSERT INTO merged_uuids (uuid, book_id, format, library_path)
              VALUES ('uuid-gone-2', 1, 'EPUB', '/books');
         INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
              VALUES (1, 'uuid-gone', 4, 1000),
                     (1, 'uuid-gone-2', 9, 2000),
                     (2, 'uuid-gone', 3, 1500);",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_merge_orphan_migration(&pool).await;

    let ratings: Vec<(i64, String, i64)> =
        sqlx::query_as("SELECT user_id, book_uuid, half_stars FROM user_ratings ORDER BY user_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        ratings,
        vec![
            (1, "uuid-live".to_string(), 9),
            (2, "uuid-live".to_string(), 3),
        ]
    );
}
