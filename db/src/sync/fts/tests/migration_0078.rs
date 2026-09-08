//! Migration 0078's create-replacement upgrade path with rows already in
//! `books_fts`: migrate to 0077, seed, then apply 0078 alone — the copy/swap
//! the full migrator never exercises on an empty database.

use super::fts_genre_hits;
use crate::test_support::count_rows;

/// The version of the migration under test.
const FTS_GENRES_VERSION: i64 = 78;

/// A pool migrated to just *below* `FTS_GENRES_VERSION` — the schema an
/// existing install sits at the moment before this upgrade lands. One
/// connection, so the in-memory database is a single shared one.
async fn pool_before_fts_genres() -> sqlx::SqlitePool {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for m in MIGRATOR.iter().filter(|m| m.version < FTS_GENRES_VERSION) {
        sqlx::raw_sql(&m.sql).execute(&pool).await.unwrap();
    }
    pool
}

/// Apply migration `FTS_GENRES_VERSION` to a pool sitting below it.
async fn apply_fts_genres_migration(pool: &sqlx::SqlitePool) {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    let m = MIGRATOR
        .iter()
        .find(|m| m.version == FTS_GENRES_VERSION)
        .expect("0078 must exist");
    sqlx::raw_sql(&m.sql).execute(pool).await.unwrap();
}

/// Read one `books_fts` column for `rowid`.
async fn fts_col(pool: &sqlx::SqlitePool, rowid: i64, col: &str) -> String {
    sqlx::query_scalar::<_, String>(&format!("SELECT {col} FROM books_fts WHERE rowid = ?"))
        .bind(rowid)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn migration_0078_preserves_indexed_override_text_while_backfilling_genres() {
    // The copy reads `books_fts`, not `books`, so the override text
    // `overlay_overrides` had written into the index survives the swap. A
    // re-derive from the canonical row would silently revert every user
    // title/tag edit in search until the next override save.
    let pool = pool_before_fts_genres().await;
    sqlx::raw_sql(
        "INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'uuid-1', 1, '/lib/a.epub', 'Scanned Title');
         INSERT INTO metadata_overrides (book_uuid, overrides)
              VALUES ('uuid-1', '{\"genres\":[\"Horror\",\"Gothic\"]}');
         INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
              VALUES (1, 'Edited Title', 'Edited Author', 'Edited Series',
                      'edited-tag', 'Edited description', '9781111111111');",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_fts_genres_migration(&pool).await;

    assert_eq!(fts_col(&pool, 1, "title").await, "Edited Title");
    assert_eq!(fts_col(&pool, 1, "authors").await, "Edited Author");
    assert_eq!(fts_col(&pool, 1, "series").await, "Edited Series");
    assert_eq!(fts_col(&pool, 1, "tags").await, "edited-tag");
    assert_eq!(fts_col(&pool, 1, "description").await, "Edited description");
    assert_eq!(fts_col(&pool, 1, "isbn").await, "9781111111111");
    assert_eq!(fts_col(&pool, 1, "genres").await, "Horror Gothic");
    assert_eq!(fts_genre_hits(&pool, "Gothic").await, 1);
}

#[tokio::test]
async fn migration_0078_recreates_the_rename_triggers_over_the_swapped_table() {
    // The three triggers name `books_fts` in their bodies, so they cannot
    // survive the table being dropped. If the recreate were missed, an
    // author rename would stop reaching the index — silently, since nothing
    // else in the schema references them.
    let pool = pool_before_fts_genres().await;
    sqlx::raw_sql(
        "INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'uuid-1', 1, '/lib/a.epub', 'A');
         INSERT INTO authors (id, name) VALUES (1, 'Olde Name');
         INSERT INTO books_authors_link (book, author) VALUES (1, 1);
         INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
              VALUES (1, 'A', 'Olde Name', '', '', '', '');",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_fts_genres_migration(&pool).await;

    let triggers = count_rows(
        &pool,
        "SELECT COUNT(*) FROM sqlite_master
          WHERE type = 'trigger' AND name IN ('books_fts_authors_rename',
                'books_fts_tags_rename', 'books_fts_series_rename')",
    )
    .await;
    assert_eq!(triggers, 3, "all three triggers must be recreated");

    sqlx::query("UPDATE authors SET name = 'New Name' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        fts_col(&pool, 1, "authors").await,
        "New Name",
        "the recreated trigger must propagate into the swapped table"
    );
}

#[tokio::test]
async fn migration_0078_survives_a_corrupt_overrides_blob() {
    // `json_each` raises `malformed JSON`, and a corrupt `overrides` row is
    // reachable state. Unguarded, one such row would abort this migration —
    // which runs at startup, so the whole install would fail to boot on
    // upgrade. The damaged row must instead converge on an empty genre index
    // without taking its neighbours down with it.
    let pool = pool_before_fts_genres().await;
    sqlx::raw_sql(
        "INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'bad-uuid', 1, '/lib/a.epub', 'A'),
                     (2, 'ok-uuid', 1, '/lib/b.epub', 'B');
         INSERT INTO metadata_overrides (book_uuid, overrides)
              VALUES ('bad-uuid', '{ not valid json'),
                     ('ok-uuid', '{\"genres\":[\"Horror\"]}');
         INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
              VALUES (1, 'A', '', '', '', '', ''), (2, 'B', '', '', '', '', '');",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_fts_genres_migration(&pool).await;

    assert_eq!(fts_col(&pool, 1, "genres").await, "");
    assert_eq!(
        fts_col(&pool, 2, "genres").await,
        "Horror",
        "a healthy neighbour must still be backfilled"
    );
}

#[tokio::test]
async fn migration_0078_skips_genres_on_an_embedded_tags_first_scan_root() {
    // `apply_overrides` returns before applying genres when the root ranks
    // embedded metadata above the override layer, so the effective metadata
    // has no genres. Seeding them anyway would make `genre:` answer for
    // books whose own detail page shows none.
    let pool = pool_before_fts_genres().await;
    sqlx::raw_sql(
        "INSERT INTO scan_roots (id, path, display_name, metadata_precedence)
              VALUES (1, '/lib', 'Lib',
                      '[\"folder_structure\",\"omnibus_overrides\",\"opf_sidecar\",\"embedded_tags\",\"provider_match\"]');
         INSERT INTO books (id, uuid, library_id, path, title)
              VALUES (1, 'uuid-1', 1, '/lib/a.epub', 'A');
         INSERT INTO metadata_overrides (book_uuid, overrides)
              VALUES ('uuid-1', '{\"genres\":[\"Horror\"]}');
         INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
              VALUES (1, 'A', '', '', '', '', '');",
    )
    .execute(&pool)
    .await
    .unwrap();

    apply_fts_genres_migration(&pool).await;

    assert_eq!(
        fts_col(&pool, 1, "genres").await,
        "",
        "override genres must not be indexed when embedded metadata outranks them"
    );
}
