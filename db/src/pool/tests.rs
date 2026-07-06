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
async fn init_db_returns_migrate_error_when_applied_checksum_is_tampered() {
    // sqlx checksums every migration and, at startup, compares each embedded
    // migration against the checksum recorded in `_sqlx_migrations`. A row
    // whose stored checksum no longer matches (an edited-after-apply migration,
    // per rule 06) must fail startup — the typed wrapper surfaces it as
    // `InitDbError::Migrate`, not a panic or a leaked `MigrateError`. Seed an
    // on-disk DB with a tampered version-1 checksum, then run `init_db` on it.
    let tmp = std::env::temp_dir().join(format!(
        "omnibus-migrate-tamper-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let url = format!("sqlite://{}?mode=rwc", tmp.display());

    // Pre-create the migrations table with sqlx's exact DDL (CREATE TABLE IF
    // NOT EXISTS, so the migrator reuses it) and record version 1 as applied
    // with a deliberately wrong checksum (48 zero bytes ≠ the real SHA-384).
    let pool = SqlitePool::connect(&url).await.unwrap();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(1_i64)
    .bind("tampered")
    .bind(true)
    .bind(vec![0u8; 48])
    .bind(0_i64)
    .execute(&pool)
    .await
    .unwrap();
    drop(pool);

    let err = init_db(&url)
        .await
        .expect_err("a tampered migration checksum must fail startup");
    let is_migrate = matches!(err, InitDbError::Migrate(_));
    let _ = std::fs::remove_file(&tmp);
    assert!(is_migrate, "expected InitDbError::Migrate, got {err:?}");
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
async fn migration_0038_stores_machine_timestamps_as_integer_unix_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Minimum FK graph, then one row per table 0038 migrated. All rely on the
    // (now epoch) column defaults except `books`, whose in-place conversion
    // dropped its default — so its two columns are set explicitly.
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'h', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, timestamp, last_modified) \
         VALUES ('bk', 1, '/lib/bk', 'Book', strftime('%s','now'), strftime('%s','now'))",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authors (id, name) VALUES (1, 'Ada')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, updated_by) VALUES ('bk', '{}', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, bytes, mime) \
         VALUES (1, 'openlibrary', 'http://x', X'00', 'image/png')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merge_log (target_book_id, source_uuid, source_metadata) \
         VALUES ((SELECT id FROM books WHERE uuid='bk'), 'src', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO ignored_authors (name) VALUES ('Bob')")
        .execute(&pool)
        .await
        .unwrap();

    // Every migrated machine-timestamp column is INTEGER-typed and a plausible
    // "now" (> 2023-11-14). `typeof` proves the storage class, not just affinity.
    for (sql, label) in [
        (
            "SELECT typeof(timestamp) || ':' || (timestamp > 1700000000) FROM books",
            "books.timestamp",
        ),
        (
            "SELECT typeof(last_modified) || ':' || (last_modified > 1700000000) FROM books",
            "books.last_modified",
        ),
        (
            "SELECT typeof(updated_at) || ':' || (updated_at > 1700000000) FROM metadata_overrides",
            "metadata_overrides.updated_at",
        ),
        (
            "SELECT typeof(fetched_at) || ':' || (fetched_at > 1700000000) FROM author_photos",
            "author_photos.fetched_at",
        ),
        (
            "SELECT typeof(merged_at) || ':' || (merged_at > 1700000000) FROM merge_log",
            "merge_log.merged_at",
        ),
        (
            "SELECT typeof(ignored_at) || ':' || (ignored_at > 1700000000) FROM ignored_authors",
            "ignored_authors.ignored_at",
        ),
    ] {
        let got: String = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
        assert_eq!(
            got, "integer:1",
            "{label} must be INTEGER unix-seconds, got {got}"
        );
    }
}

#[tokio::test]
async fn migration_0038_text_datetime_converts_to_exact_unix_seconds() {
    // The in-DDL backfill relies on this exact conversion of the old
    // `datetime('now')` / `CURRENT_TIMESTAMP` TEXT form. Locking it here is the
    // parsing case rule 03 requires; it would fail on the pre-0038 schema where
    // these columns stayed TEXT and sorted lexicographically.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let epoch: i64 =
        sqlx::query_scalar("SELECT CAST(strftime('%s','2024-01-02 03:04:05') AS INTEGER)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(epoch, 1_704_164_645);
}

#[tokio::test]
async fn migration_0038_recreate_drops_orphan_author_photos() {
    // Real databases carry rows that violate a declared FK because the parent
    // was removed while enforcement was off — e.g. GC'd authors leaving orphan
    // `author_photos`. 0038's table recreates re-validate every FK under
    // enforcement, so the copy must repair such orphans (drop the dead
    // CASCADE rows) instead of aborting with `FOREIGN KEY constraint failed`.
    // `init_db` runs 0038 atomically on an empty DB and can't hold a
    // pre-migration orphan, so this reconstructs the minimal shape and drives
    // the same orphan-filtering recreate pattern 0038 uses.
    //
    // A pool over `sqlite::memory:` hands each connection its own DB, so every
    // statement must run on one acquired connection.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE author_photos (author_id INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE, \
         source TEXT NOT NULL, url TEXT, bytes BLOB, mime TEXT, fetched_at TEXT NOT NULL DEFAULT (datetime('now')))",
        "INSERT INTO authors (id, name) VALUES (1, 'Ada')",
        "INSERT INTO author_photos (author_id, source) VALUES (1, 'letter')",
        // Orphan: author 99 doesn't exist. Insert under FK-off, mimicking a
        // parent deletion that ran without cascade enforcement.
        "PRAGMA foreign_keys=OFF",
        "INSERT INTO author_photos (author_id, source) VALUES (99, 'letter')",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE author_photos_new (author_id INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE, \
         source TEXT NOT NULL, url TEXT, bytes BLOB, mime TEXT, fetched_at INTEGER NOT NULL DEFAULT (strftime('%s','now')))",
        "INSERT INTO author_photos_new \
           SELECT author_id, source, url, bytes, mime, CAST(strftime('%s', fetched_at) AS INTEGER) \
             FROM author_photos ap WHERE EXISTS (SELECT 1 FROM authors a WHERE a.id = ap.author_id)",
        "DROP TABLE author_photos",
        "ALTER TABLE author_photos_new RENAME TO author_photos",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-filtering recreate must not hit a FK violation");
    }

    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT author_id FROM author_photos ORDER BY author_id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(ids, vec![1], "orphan row dropped, valid row kept");
}

#[tokio::test]
async fn migration_0038_recreate_coerces_orphan_metadata_updated_by_to_null() {
    // `metadata_overrides.updated_by` is `ON DELETE SET NULL`: a dangling
    // reference (user deleted without enforcement) must become NULL on the
    // recreate's `LEFT JOIN users`, keeping the row rather than aborting.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT)",
        "CREATE TABLE metadata_overrides (book_uuid TEXT NOT NULL PRIMARY KEY, overrides TEXT NOT NULL DEFAULT '{}', \
         has_cover_override INTEGER NOT NULL DEFAULT 0, updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
        "INSERT INTO users (id, username) VALUES (1, 'admin')",
        "INSERT INTO metadata_overrides (book_uuid, updated_by) VALUES ('valid', 1)",
        "PRAGMA foreign_keys=OFF",
        "INSERT INTO metadata_overrides (book_uuid, updated_by) VALUES ('orphan', 99)",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE metadata_overrides_new (book_uuid TEXT NOT NULL PRIMARY KEY, overrides TEXT NOT NULL DEFAULT '{}', \
         has_cover_override INTEGER NOT NULL DEFAULT 0, updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')))",
        "INSERT INTO metadata_overrides_new \
           SELECT mo.book_uuid, mo.overrides, mo.has_cover_override, u.id, CAST(strftime('%s', mo.updated_at) AS INTEGER) \
             FROM metadata_overrides mo LEFT JOIN users u ON u.id = mo.updated_by",
        "DROP TABLE metadata_overrides",
        "ALTER TABLE metadata_overrides_new RENAME TO metadata_overrides",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-coercing recreate must not hit a FK violation");
    }

    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT book_uuid, updated_by FROM metadata_overrides ORDER BY book_uuid")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![("orphan".into(), None), ("valid".into(), Some(1))],
        "both rows kept; the dangling updated_by is coerced to NULL"
    );
}

#[tokio::test]
async fn migration_0038_recreate_drops_orphan_merge_log_and_coerces_merged_by() {
    // `merge_log.target_book_id` is NOT NULL CASCADE — an orphan (book gone)
    // must be dropped by the inner `JOIN books`. `merged_by` is SET NULL — a
    // dangling ref must survive as NULL via the `LEFT JOIN users`.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT)",
        "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT)",
        "CREATE TABLE merge_log (id INTEGER PRIMARY KEY AUTOINCREMENT, \
         target_book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, source_uuid TEXT NOT NULL, \
         source_metadata TEXT NOT NULL, merged_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         merged_at TEXT NOT NULL DEFAULT (datetime('now')), undone_at TEXT)",
        "INSERT INTO users (id, username) VALUES (1, 'admin')",
        "INSERT INTO books (id, title) VALUES (1, 'Book')",
        // Valid row (target 1, merged_by 1).
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (1, 1, 'a', '{}', 1)",
        "PRAGMA foreign_keys=OFF",
        // Orphan target (book 99 gone) -> dropped; dangling merged_by (user 99) -> coerced.
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (2, 99, 'b', '{}', 1)",
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (3, 1, 'c', '{}', 99)",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE merge_log_new (id INTEGER PRIMARY KEY AUTOINCREMENT, \
         target_book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, source_uuid TEXT NOT NULL, \
         source_metadata TEXT NOT NULL, merged_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         merged_at INTEGER NOT NULL DEFAULT (strftime('%s','now')), undone_at INTEGER)",
        "INSERT INTO merge_log_new \
           SELECT ml.id, ml.target_book_id, ml.source_uuid, ml.source_metadata, u.id, \
                  CAST(strftime('%s', ml.merged_at) AS INTEGER), \
                  CASE WHEN ml.undone_at IS NULL THEN NULL ELSE CAST(strftime('%s', ml.undone_at) AS INTEGER) END \
             FROM merge_log ml JOIN books b ON b.id = ml.target_book_id LEFT JOIN users u ON u.id = ml.merged_by",
        "DROP TABLE merge_log",
        "ALTER TABLE merge_log_new RENAME TO merge_log",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-repairing recreate must not hit a FK violation");
    }

    let rows: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, merged_by FROM merge_log ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![(1, Some(1)), (3, None)],
        "orphan-target row 2 dropped; row 3's dangling merged_by coerced to NULL"
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
