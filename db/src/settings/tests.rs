//! Unit tests for the `settings` module — `get_settings`/`set_settings`
//! roundtrip, env-var seeding (serialized via a process-global guard),
//! F2 repoint-in-place identity preservation, and never-prune retention.

use super::*;
use crate::books::list_books;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir, EnvVarGuard};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_settings_returns_none_for_empty_db() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let settings = get_settings(&pool).await.unwrap();
    assert_eq!(settings.ebook_library_path, None);
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn set_and_get_settings_roundtrips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let input = Settings {
        ebook_library_path: Some("/books/ebooks".into()),
        audiobook_library_path: Some("/books/audio".into()),
    };
    set_settings(&pool, &input).await.unwrap();
    assert_eq!(get_settings(&pool).await.unwrap(), input);
}

#[tokio::test]
async fn set_settings_updates_existing_values() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/old".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/new".into()),
            audiobook_library_path: Some("/audio".into()),
        },
    )
    .await
    .unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, Some("/new".into()));
    assert_eq!(result.audiobook_library_path, Some("/audio".into()));
}

#[tokio::test]
async fn set_settings_none_clears_existing_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
        },
    )
    .await
    .unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, None);
    assert_eq!(result.audiobook_library_path, None);
}

#[tokio::test]
async fn seed_settings_from_env_writes_env_vars_to_db() {
    // The guards serialize the process-global env mutation against the
    // other `seed_settings_from_env` test and restore prior values on
    // drop, so this test can't leak `EBOOK_LIBRARY_PATH` into the rest of
    // the run.
    let _env = EnvVarGuard::set("EBOOK_LIBRARY_PATH", Some("/env/books"))
        .also_set("AUDIOBOOK_LIBRARY_PATH", Some("/env/audio"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_settings_from_env(&pool).await.unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, Some("/env/books".into()));
    assert_eq!(result.audiobook_library_path, Some("/env/audio".into()));
}

#[tokio::test]
async fn seed_settings_from_env_is_noop_when_vars_unset() {
    // Guard restores prior values on drop. Both vars are removed to
    // establish the "unset" precondition this test exercises.
    let _env =
        EnvVarGuard::set("EBOOK_LIBRARY_PATH", None).also_set("AUDIOBOOK_LIBRARY_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_settings_from_env(&pool).await.unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, None);
    assert_eq!(result.audiobook_library_path, None);
}

// ── Hardcover API key (F3.3) — settings wins, env seeds-if-unset ──

#[tokio::test]
async fn hardcover_key_set_get_and_clear_roundtrips() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);

    set_hardcover_api_key(&pool, Some("  hc_secret  "))
        .await
        .unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("hc_secret")
    );

    // Blank clears (treated as None).
    set_hardcover_api_key(&pool, Some("   ")).await.unwrap();
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_hardcover_api_key_accepts_value_at_max_len() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let at_limit = "a".repeat(HARDCOVER_API_KEY_MAX_LEN);
    set_hardcover_api_key(&pool, Some(&at_limit)).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some(at_limit.as_str())
    );
}

#[tokio::test]
async fn set_hardcover_api_key_rejects_value_over_max_len() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let over_limit = "a".repeat(HARDCOVER_API_KEY_MAX_LEN + 1);
    let err = set_hardcover_api_key(&pool, Some(&over_limit))
        .await
        .expect_err("over-limit value should be rejected");
    match &err {
        SettingsError::Validation(msg) => assert!(
            msg.contains(&HARDCOVER_API_KEY_MAX_LEN.to_string()),
            "error message should name the cap: {msg}"
        ),
        other => panic!("expected SettingsError::Validation, got {other:?}"),
    }
    // Validation must short-circuit before the KV write.
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn effective_hardcover_key_prefers_saved_over_env() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    assert_eq!(
        effective_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn effective_hardcover_key_falls_back_to_env_when_unset() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(
        effective_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("env-key")
    );
}

#[tokio::test]
async fn effective_hardcover_key_is_none_when_neither_set() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(effective_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn seed_hardcover_key_seeds_only_when_unset() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    // No saved value → env seeds it.
    seed_hardcover_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("env-key")
    );

    // A subsequent settings save wins, and re-seeding does NOT clobber it.
    set_hardcover_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    seed_hardcover_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn hardcover_key_status_reports_settings_source_masked() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("abcdefghijkl"))
        .await
        .unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Long keys collapse to first4…last4 — never the raw value.
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn hardcover_key_status_falls_back_to_env_masked() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("abcdefghijkl"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "env");
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn hardcover_key_status_masks_short_keys_as_bullets() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("short")).await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Keys of 8 chars or fewer never leak length beyond the fixed bullet run.
    assert_eq!(
        status.masked.as_deref(),
        Some("\u{2022}\u{2022}\u{2022}\u{2022}")
    );
}

#[tokio::test]
async fn hardcover_key_status_is_unset_when_neither_set() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(!status.configured);
    assert_eq!(status.source, "none");
    assert_eq!(status.masked, None);
}

/// F2 repoint-in-place: changing the ebook path moves the existing
/// `scan_roots` row (keeping its id) so every book — and its durable
/// `books.uuid` — survives the move under the new path. Nothing is deleted.
#[tokio::test]
async fn set_settings_repoints_library_in_place_preserving_identity() {
    let _covers = CoversTempDir::new("repoint");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/old".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/old",
        vec![indexed(
            "a.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let before = list_books(&pool, "/old").await.unwrap();
    assert_eq!(before.len(), 1);
    let uuid_before = before[0].unique_identifier.clone();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/new".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();

    // The old path is empty; the book now lists under the new path with its
    // identity intact, and exactly one scan_roots row / one book survive.
    assert!(list_books(&pool, "/old").await.unwrap().is_empty());
    let after = list_books(&pool, "/new").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].unique_identifier, uuid_before,
        "books.uuid must be preserved across a repoint"
    );
    let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(library_count, 1, "scan_roots row repointed, not recreated");
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "never-prune: the book is retained");
}
#[tokio::test]
async fn set_settings_keeps_libraries_still_configured() {
    let _covers = CoversTempDir::new("prune-keep");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/books",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
        },
    )
    .await
    .unwrap();

    assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
}
/// Never-prune (F2): clearing the ebook path does **not** delete the
/// library's books (the durable-identity safety net) — its `scan_roots` row
/// and books are retained, and re-adding the path lists them again.
#[tokio::test]
async fn set_settings_none_retains_library_data() {
    let _covers = CoversTempDir::new("prune-clear");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/books",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();

    // The scan_roots row still owns a book, so never-prune keeps both.
    let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        library_count, 1,
        "never-prune retains a non-empty scan root"
    );
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "books are retained when the path is cleared");
    // Re-adding the path surfaces the retained book again.
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
}
/// Never-prune (F2): `prune_orphan_libraries` drops only **childless**
/// orphan scan roots; a root not in `keep` that still owns books is retained
/// (its books and their soft-ref user data must never be cascade-deleted),
/// and it returns no cover uuids (nothing is deleted).
#[tokio::test]
async fn prune_orphan_libraries_keeps_non_empty_roots_and_drops_childless() {
    let _covers = CoversTempDir::new("prune-never");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Root A: orphaned but owns a book → must be retained.
    let a_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind("/with-book")
    .bind("A")
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) VALUES (?, ?, ?, ?, ?)",
    )
    .bind("uuid-a")
    .bind("book.epub")
    .bind(a_id)
    .bind("/with-book")
    .bind("Kept")
    .execute(&pool)
    .await
    .unwrap();
    // Root B: orphaned and childless → must be dropped.
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind("/childless")
        .bind("B")
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let orphan_uuids = prune_orphan_libraries(&mut tx, &[]).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        orphan_uuids.is_empty(),
        "never-prune deletes no books, so no cover uuids are returned"
    );
    let roots: Vec<String> = sqlx::query_scalar("SELECT path FROM scan_roots ORDER BY path")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        roots,
        vec!["/with-book".to_string()],
        "childless orphan dropped, book-owning orphan retained"
    );
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "the orphaned-root book is never deleted");
}

/// Migration 0019 renames `libraries` -> `scan_roots` via
/// `ALTER TABLE ... RENAME`. SQLite auto-rewrites the `books.library_id`
/// FK to reference the renamed table, so deleting a scan-root row must
/// still cascade-delete its books. A bug in the rename (e.g. a stray
/// table recreate that dropped the FK) would leave the book row behind.
#[tokio::test]
async fn fk_cascade_survives_libraries_rename_to_scan_roots() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let scan_root_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind("/lib")
    .bind("lib")
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?)")
        .bind("uuid-cascade")
        .bind(scan_root_id)
        .bind("/lib/book.epub")
        .bind("Cascade Book")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM scan_roots WHERE id = ?")
        .bind(scan_root_id)
        .execute(&pool)
        .await
        .unwrap();

    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE library_id = ?")
        .bind(scan_root_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_count, 0,
        "deleting the scan_roots row should cascade-delete its books \
         after the libraries->scan_roots rename"
    );
}

/// Shared-root guard (F2): when ebook + audiobook point at the same scan
/// root and only one slot's path changes, the shared `scan_roots` row must
/// NOT be repointed out from under the slot that still uses it (that would
/// silently move both libraries).
#[tokio::test]
async fn set_settings_does_not_repoint_a_shared_root_for_a_single_slot_change() {
    let _covers = CoversTempDir::new("shared-root");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: Some("/lib".into()),
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    // Change only the ebook slot; the audiobook slot still points at "/lib".
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/newlib".into()),
            audiobook_library_path: Some("/lib".into()),
        },
    )
    .await
    .unwrap();

    // The shared row is untouched (not renamed to /newlib), so the slot that
    // still uses "/lib" keeps resolving its books — nothing was moved silently.
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM scan_roots ORDER BY path")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        paths,
        vec!["/lib".to_string()],
        "a shared root must not be repointed for a single-slot change"
    );
    assert_eq!(list_books(&pool, "/lib").await.unwrap().len(), 1);
}
