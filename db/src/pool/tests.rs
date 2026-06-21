use super::*;

#[tokio::test]
async fn init_db_returns_db_error_when_url_is_invalid() {
    // `sqlite://` URL pointing at a path under a non-existent dir + no
    // `mode=rwc` flag forces the underlying pool connect to fail. The
    // typed wrapper must surface a `Db` variant rather than panicking
    // or leaking `sqlx::Error` at the signature.
    let err = init_db("sqlite:///nonexistent/dir/omnibus.db")
        .await
        .expect_err("invalid url should fail to open");
    assert!(matches!(err, InitDbError::Db(_)));
}

#[tokio::test]
async fn migrator_records_applied_versions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("_sqlx_migrations should exist after init_db");
    assert!(
        versions.contains(&1),
        "baseline migration 0001 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&2),
        "normalized migration 0002 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&3),
        "legacy-drop migration 0003 should be recorded, got {versions:?}"
    );
}

#[tokio::test]
async fn migration_0021_drops_redundant_and_dead_schema_objects() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // F17 + F18: the four redundant indexes and the speculative
    // NULL-accent partial index must be gone after the migrator runs.
    for index in [
        "idx_books_uuid",
        "idx_book_files_book_id",
        "idx_book_file_parts_lookup",
        "reading_progress_user_book_idx",
        "idx_books_accent_null",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?")
                .bind(index)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "index {index} should have been dropped by 0021");
    }

    // F20: the vestigial single-row table must be gone.
    let app_state: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_state'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        app_state, 0,
        "app_state table should have been dropped by 0021"
    );

    // The covering composite index that supersedes idx_book_files_book_id
    // must still be present (kept, not dropped).
    let kept_composite: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_book_files_book_format'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        kept_composite, 1,
        "idx_book_files_book_format must survive — it covers book_id-only lookups"
    );

    // The (user_id, book_id) session indexes from 0013 have no covering
    // UNIQUE, so they must NOT have been dropped.
    for kept in [
        "bookmarks_user_book_idx",
        "reading_sessions_user_book_idx",
        "listening_sessions_user_book_idx",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?")
                .bind(kept)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "index {kept} has no covering UNIQUE and must be kept"
        );
    }

    // The UNIQUE(user_id, book_uuid, format) auto-index (the soft-ref key
    // post-F1) must still enforce uniqueness: a duplicate
    // (user_id, book_uuid, format) insert must fail.
    sqlx::query("INSERT INTO users (username, password_hash, is_admin) VALUES ('u', 'h', 0)")
        .execute(&pool)
        .await
        .unwrap();
    // `libraries` was renamed to `scan_roots` in 0019; `books.library_id`
    // keeps its column name and FK.
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('bk-uuid', 1, '/lib/bk', 'Book')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi) \
         VALUES (1, 'bk-uuid', 'epub', 'cfi-1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dup = sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi) \
         VALUES (1, 'bk-uuid', 'epub', 'cfi-2')",
    )
    .execute(&pool)
    .await;
    assert!(
        dup.is_err(),
        "UNIQUE(user_id, book_uuid, format) must still reject a duplicate row"
    );
}

#[tokio::test]
async fn migrator_is_idempotent_on_rerun() {
    let tmp = std::env::temp_dir().join(format!(
        "omnibus-migrate-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let url = format!("sqlite://{}?mode=rwc", tmp.display());

    let pool1 = init_db(&url).await.expect("first init");
    drop(pool1);
    let pool2 = init_db(&url).await.expect("second init");

    let by_version: Vec<(i64, i64)> =
        sqlx::query_as("SELECT version, COUNT(*) FROM _sqlx_migrations GROUP BY version")
            .fetch_all(&pool2)
            .await
            .unwrap();
    for (_, count) in by_version {
        assert_eq!(count, 1, "every migration recorded exactly once");
    }

    drop(pool2);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn purge_legacy_covers_once_sweeps_then_no_ops() {
    // Standalone temp dir so we don't depend on CoversTempDir's env var
    // (purge_legacy_covers_once takes the dir as a parameter, and we
    // want to assert the function in isolation from init_db).
    let pid = std::process::id();
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("omnibus_purge_test_{pid}_{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Seed three "legacy" cover files.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    purge_legacy_covers_once(&dir);

    // Legacy files gone, sentinel written.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        assert!(
            !dir.join(name).exists(),
            "legacy file {name} should have been purged",
        );
    }
    assert!(
        dir.join(COVERS_SCHEME_SENTINEL).exists(),
        "sentinel should be present after first purge",
    );

    // A freshly-written cover after the purge must survive a second
    // call — the sentinel short-circuits the sweep.
    let kept = dir.join("dddd.jpg");
    std::fs::write(&kept, b"y").unwrap();
    purge_legacy_covers_once(&dir);
    assert!(
        kept.exists(),
        "post-sentinel cover writes must not be deleted",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn purge_legacy_covers_once_handles_missing_dir() {
    // Cold-boot before any covers have ever been written — must not panic
    // and must not create the directory.
    let pid = std::process::id();
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("omnibus_purge_missing_{pid}_{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    purge_legacy_covers_once(&dir);
    assert!(
        !dir.exists(),
        "purge must not create the covers dir as a side effect",
    );
}
