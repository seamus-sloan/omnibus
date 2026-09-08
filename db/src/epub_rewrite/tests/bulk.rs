//! `rewrite_all_epubs_with_overrides`: only books with active overrides
//! are rewritten, a per-book error is recorded without aborting the run,
//! the zero summary, the DB-failure path, and a query count independent
//! of the book count.

use omnibus_shared::MetadataOverrides;

use super::seed_epub_row;
use crate::ebook::test_support::copy_fixture_into;
use crate::test_support::EnvVarGuard;

/// Insert a `books` row with no `book_files` row at all — an audiobook-only
/// (or otherwise EPUB-less) book, for the "skipped: no EPUB" branch.
async fn seed_epubless_book(pool: &sqlx::SqlitePool, uuid: &str, title: &str) -> i64 {
    let lib_id =
        sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/audio', 'audio')")
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?)")
        .bind(uuid)
        .bind(lib_id)
        .bind(title)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn rewrite_all_epubs_with_overrides_rewrites_only_books_with_active_overrides() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Book A: an EPUB plus an active override → rewritten.
    let lib_a = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib_a.path());
    seed_epub_row(&pool, lib_a.path(), "uuid-a", "Book A", "alpha").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-a",
        &MetadataOverrides {
            title: Some("Overridden A".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // Book B: an EPUB but no override row at all → excluded entirely, never
    // touched (not counted as rewritten, skipped, or errored).
    let lib_b = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib_b.path());
    seed_epub_row(&pool, lib_b.path(), "uuid-b", "Book B", "alpha").await;

    // Book C: audiobook-only, but with an active override → skipped, since
    // there's no EPUB to bake it into.
    seed_epubless_book(&pool, "uuid-c", "Book C").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-c",
        &MetadataOverrides {
            title: Some("Overridden C".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let progress = std::sync::Mutex::new(Vec::new());
    let summary =
        super::super::rewrite_all_epubs_with_progress(&pool, |processed, total, current| {
            progress
                .lock()
                .unwrap()
                .push((processed, total, current.map(str::to_string)));
        })
        .await
        .unwrap();

    assert_eq!(summary.rewritten, 1, "{summary:?}");
    assert_eq!(summary.skipped, 1, "{summary:?}");
    assert!(summary.errors.is_empty(), "{summary:?}");

    // One progress call per override row (A and C — B has no override), each
    // naming the book being baked. The metadata fetch merges overrides, so
    // A reports its overridden title. Order follows the override listing,
    // so only membership is asserted.
    let calls = progress.into_inner().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls.iter().all(|(_, total, _)| *total == 2), "{calls:?}");
    let names: Vec<Option<String>> = calls.into_iter().map(|(_, _, n)| n).collect();
    assert!(
        names.contains(&Some("Overridden A".to_string()))
            && names.contains(&Some("Overridden C".to_string())),
        "{names:?}"
    );
}

#[tokio::test]
async fn rewrite_all_epubs_with_overrides_records_a_per_book_error_without_aborting_the_run() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Book "ok": a real fixture on disk → rewritten successfully.
    let lib_ok = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib_ok.path());
    seed_epub_row(&pool, lib_ok.path(), "uuid-ok", "Book OK", "alpha").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-ok",
        &MetadataOverrides {
            title: Some("Fixed".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // Book "bad": the `book_files` row points at a file that was never
    // written to disk, so the source is missing when the rewrite opens it.
    let lib_bad = tempfile::tempdir().unwrap();
    seed_epub_row(&pool, lib_bad.path(), "uuid-bad", "Book Bad", "missing").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-bad",
        &MetadataOverrides {
            title: Some("Never Applied".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let summary = super::super::rewrite_all_epubs_with_overrides(&pool)
        .await
        .unwrap();

    assert_eq!(summary.rewritten, 1, "{summary:?}");
    assert_eq!(summary.skipped, 0, "{summary:?}");
    assert_eq!(summary.errors.len(), 1, "{summary:?}");
    assert_eq!(summary.errors[0].book_uuid, "uuid-bad");
    assert!(!summary.errors[0].message.is_empty());
}

#[tokio::test]
async fn rewrite_all_epubs_with_overrides_returns_a_zero_summary_when_no_overrides_exist() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let summary = super::super::rewrite_all_epubs_with_overrides(&pool)
        .await
        .unwrap();
    assert_eq!(summary.rewritten, 0);
    assert_eq!(summary.skipped, 0);
    assert!(summary.errors.is_empty());
}

#[tokio::test]
async fn rewrite_all_epubs_with_overrides_propagates_a_db_error_when_pool_is_closed() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = super::super::rewrite_all_epubs_with_overrides(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("list overridden book uuids"));
}

/// Counts `tracing` events sqlx emits (target `"sqlx::query"`, one per
/// executed statement) while installed as the default subscriber. Every
/// other `Subscriber` method is a no-op — this only needs to tally events,
/// never spans.
struct QueryCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for QueryCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::test]
async fn rewrite_all_epubs_with_overrides_issues_a_query_count_independent_of_book_count() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Seed a batch of overridden books whose EPUB source is never written to
    // disk — this test only cares about DB round trips (AC1), not the
    // (file-only) bake itself, so a nonexistent source is fine: every book
    // still resolves through the batched id/path/last-modified lookups
    // before its blocking rewrite fails. All books share one `scan_roots`
    // row (inserted once, `seed_epub_row`-style repeated inserts would
    // collide on its `UNIQUE path`).
    const BOOK_COUNT: usize = 30;
    let lib = tempfile::tempdir().unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib.path().to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    for i in 0..BOOK_COUNT {
        let uuid = format!("uuid-{i}");
        let book_id =
            sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?)")
                .bind(&uuid)
                .bind(lib_id)
                .bind(format!("Book {i}"))
                .execute(&pool)
                .await
                .unwrap()
                .last_insert_rowid();
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes) \
             VALUES (?, 'EPUB', ?, 0)",
        )
        .bind(book_id)
        .bind(format!("missing-{i}"))
        .execute(&pool)
        .await
        .unwrap();
        crate::upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some(format!("Overridden {i}")),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
    }

    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guard = tracing::subscriber::set_default(QueryCounter(count.clone()));
    let summary = super::super::rewrite_all_epubs_with_overrides(&pool)
        .await
        .unwrap();
    drop(guard);

    assert_eq!(
        summary.errors.len(),
        BOOK_COUNT,
        "every book's source is missing on disk: {summary:?}"
    );
    let queries = count.load(std::sync::atomic::Ordering::SeqCst);
    // AC1: the batched resolve path issues a small, book-count-independent
    // number of chunked queries — well under one per book, let alone the old
    // design's up to 4+ round trips per book PLUS `get_book`'s own several
    // internal queries (overrides, precedence, creator-id backfill, file
    // list) for every one of them.
    assert!(
        queries < BOOK_COUNT,
        "expected a query count independent of book count, got {queries} \
         queries for {BOOK_COUNT} books"
    );
}
