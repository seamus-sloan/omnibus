//! The export cache: `rewritten_epub_path` with and without overrides,
//! staleness, leftover-file cleanup, the unknown-book, missing-source and
//! DB-failure errors, the cap read from the environment, invalidation,
//! and FIFO eviction that skips in-flight temp files.

use epub::doc::EpubDoc;
use omnibus_shared::MetadataOverrides;

use super::epub_title;
use crate::ebook::test_support::fixture;
use crate::test_support::EnvVarGuard;

#[tokio::test]
async fn rewritten_epub_path_returns_none_without_overrides() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let src = fixture("alpha.epub");
    let out = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap();
    assert!(out.is_none(), "no overrides → serve source, no rewrite");
}

#[tokio::test]
async fn rewritten_epub_path_bakes_title_override_into_export() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let overrides = omnibus_shared::MetadataOverrides {
        title: Some("Stormlight #1".into()),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, user_id)
        .await
        .unwrap();

    let src = fixture("alpha.epub");
    let out = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap()
        .expect("override present → rewritten export");

    let doc = EpubDoc::new(&out).unwrap();
    assert_eq!(epub_title(&doc).as_deref(), Some("Stormlight #1"));

    // Second call is idempotent — returns the same cached path.
    let again = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, out);
}

#[tokio::test]
async fn is_stale_returns_true_when_export_missing() {
    assert!(super::super::is_stale(std::path::Path::new("/nonexistent/x.epub"), 0).await);
}

/// #1395: once a book's last override clears, `rewritten_epub_path` must
/// remove any cache file left behind rather than leaving it orphaned. This
/// exercises the belt-and-suspenders cleanup on the early-return path
/// directly — the eager cleanup in `metadata_overrides::delete_metadata_overrides`
/// / `clear_cover_override` is covered separately in that module's tests.
#[tokio::test]
async fn rewritten_epub_path_removes_a_leftover_cache_file_when_no_override_is_active() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "wok.epub", "The Way of Kings", "Sanderson")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    // Simulate an orphan left behind by a pre-#1395 build: a cache file with
    // no override active to justify it.
    let cache_path = super::super::export_epub_path(id);
    std::fs::write(&cache_path, b"orphaned rewritten epub").unwrap();

    let src = fixture("alpha.epub");
    let out = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap();

    assert!(out.is_none(), "no overrides → serve source, no rewrite");
    assert!(
        !cache_path.exists(),
        "the orphaned cache file must be cleaned up rather than left on disk"
    );
}

#[tokio::test]
async fn rewritten_epub_path_returns_an_error_for_unknown_book_id() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let src = fixture("alpha.epub");

    let err = super::super::rewritten_epub_path(&pool, 9999, &src)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("book 9999 not found"));
}

#[tokio::test]
async fn rewritten_epub_path_propagates_a_db_error_when_pool_is_closed() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let uuid =
        crate::test_support::seed_synced_ebook(&pool, "closed-pool.epub", "Closed Pool", "Author")
            .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();
    pool.close().await;

    let src = fixture("alpha.epub");
    let err = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap_err();
    // Nothing downstream branches on the concrete error type any more — the
    // get_book lookup's `BooksError` still rides along in the anyhow source
    // chain for the server log to surface. Check the whole chain rather than
    // just the top frame, so this stays true if a future `.context(...)` is
    // added around the lookup.
    assert!(err
        .chain()
        .any(|e| e.downcast_ref::<crate::books::BooksError>().is_some()));
}

#[tokio::test]
async fn rewritten_epub_path_returns_an_error_when_the_source_epub_is_missing() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(
        &pool,
        "missing-source.epub",
        "Missing Source",
        "Author",
    )
    .await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    let overrides = MetadataOverrides {
        title: Some("New Title".into()),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, user_id)
        .await
        .unwrap();

    // An override is active, so `rewritten_epub_path` attempts the rewrite —
    // but `source` doesn't exist on disk, so opening it as an epub fails.
    let src = export.path().join("does-not-exist.epub");
    let err = super::super::rewritten_epub_path(&pool, id, &src)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("open source epub"));
}

// Note: `EnvVarGuard` holds the process-wide `ENV_LOCK` for its lifetime and
// that mutex is not reentrant, so each test below keeps at most one guard
// alive at a time — two in one scope self-deadlocks the test binary.
#[test]
fn cap_bytes_falls_back_to_default_when_env_is_unset() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", None);
    assert_eq!(super::super::cap_bytes(), 5 * 1024 * 1024 * 1024);
}

#[test]
fn cap_bytes_falls_back_to_default_when_env_is_unparseable() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", Some("not-a-number"));
    assert_eq!(super::super::cap_bytes(), 5 * 1024 * 1024 * 1024);
}

#[test]
fn cap_bytes_reads_the_configured_env_var() {
    let _env = EnvVarGuard::set("OMNIBUS_EXPORT_EPUB_CAP_BYTES", Some("1024"));
    assert_eq!(super::super::cap_bytes(), 1024);
}

#[test]
fn invalidate_export_epub_cache_removes_an_existing_file_and_is_a_noop_when_absent() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let path = super::super::export_epub_path(42);
    std::fs::write(&path, b"cached").unwrap();
    assert!(path.exists());

    super::super::invalidate_export_epub_cache(42);
    assert!(!path.exists());

    // A second call with nothing left to remove must not panic or error.
    super::super::invalidate_export_epub_cache(42);
}

#[test]
fn evict_if_over_cap_removes_oldest_entries_until_under_the_cap() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    // Three 10-byte entries, oldest (book 1) to newest (book 3).
    for (book_id, age_secs) in [(1, 30), (2, 20), (3, 10)] {
        let path = super::super::export_epub_path(book_id);
        std::fs::write(&path, b"0123456789").unwrap();
        let mtime = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        let file = std::fs::File::open(&path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    // Cap fits only the newest two entries (20 bytes).
    super::super::evict_if_over_cap(20).unwrap();

    assert!(
        !super::super::export_epub_path(1).exists(),
        "oldest entry should have been evicted"
    );
    assert!(super::super::export_epub_path(2).exists());
    assert!(super::super::export_epub_path(3).exists());
}

#[test]
fn evict_if_over_cap_ignores_in_flight_temp_files() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    // A temp file mid-rewrite (dot-prefixed, per `rewritten_epub_path`'s
    // naming scheme) must not be counted or deleted by eviction.
    let tmp = export.path().join(".7.12345.0.tmp.epub");
    std::fs::write(&tmp, b"in-flight").unwrap();

    super::super::evict_if_over_cap(0).unwrap();

    assert!(tmp.exists(), "in-flight temp file must survive eviction");
}

#[test]
fn evict_if_over_cap_is_a_noop_when_the_export_dir_does_not_exist() {
    let export = tempfile::tempdir().unwrap();
    let missing = export.path().join("does-not-exist");
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(missing.as_os_str()));

    super::super::evict_if_over_cap(0).unwrap();
}
