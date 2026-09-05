//! `Task::Scan` and the follow-ups it posts: the ghost-file warning bands,
//! the thumbnail backfill (pre-generation and freshness skip), the cleanup
//! detection task and its `dedup_suggestions` rows, and which task kinds
//! count as resizing the library.

use omnibus_shared::GhostFilesWarning;
use sqlx::SqlitePool;

use crate::ebook::test_support::copy_fixture_into;
use crate::sync::{sync_books, SyncPlan};
use crate::test_support::{indexed_with_stat, make_test_dir, EnvVarGuard};

use super::super::types::{Task, TaskOutcome, TaskSuccessDetail};
use super::{make_worker_default, poll_maps_empty, pool};

// `periodic_scan_tick` tests moved to `worker::periodic_scan::tests`, a
// sibling of `periodic_scan.rs`.
/// Seed `count` on-disk stub ebooks under `library_path` through the real
/// `sync_books` write path, each with its actual on-disk `(mtime, size)`
/// stat so a later `Task::Scan` classifies an untouched file as Unchanged.
async fn seed_stub_ebooks(pool: &SqlitePool, library_path: &str, count: usize) {
    for i in 0..count {
        let filename = format!("book-{i}.epub");
        let abs = std::path::Path::new(library_path).join(&filename);
        std::fs::write(&abs, b"not a zip").unwrap();
        let meta = std::fs::metadata(&abs).unwrap();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let book = indexed_with_stat(&filename, Some(&filename), mtime, meta.len() as i64);
        sync_books(
            pool,
            library_path,
            SyncPlan {
                new_books: vec![book],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
}

/// AC2: a scan that ghosts only a few books (below the warn threshold)
/// completes with the ordinary `Done { ghost_warning: None }` — the
/// existing `DoneRow` behavior is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_reports_no_ghost_warning_below_the_warn_threshold() {
    let pool = pool().await;
    let lib = make_test_dir("worker-scan-no-warning");
    let library_path = lib.to_string_lossy().into_owned();
    seed_stub_ebooks(&pool, &library_path, 20).await;
    // 3 of 20 ghosted books is under MASS_MISSING_MIN_ABSOLUTE (10) — always
    // silent, regardless of the 15% fraction.
    for i in 0..3 {
        std::fs::remove_file(lib.join(format!("book-{i}.epub"))).unwrap();
    }

    let w = make_worker_default(pool);
    let id = w.post(Task::Scan { library_path });
    match w.await_completion(id).await {
        TaskOutcome::Ok(detail) => assert_eq!(detail, None),
        other => panic!("expected Ok, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC1/AC5: a scan that ghosts a large-but-sub-abort-threshold number of
/// books completes successfully (the #819 abort guard does not trip) but
/// its `Done` state carries a [`GhostFilesWarning`] naming the ghost count
/// and the pre-scan file-backed total — the wire type the settings page
/// renders a distinct warning row from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_reports_ghost_warning_in_the_warn_band_below_abort() {
    let pool = pool().await;
    let lib = make_test_dir("worker-scan-warning-band");
    let library_path = lib.to_string_lossy().into_owned();
    seed_stub_ebooks(&pool, &library_path, 100).await;
    // 15 of 100 (15%) clears the 10% warn fraction but stays under the 20%
    // abort fraction — the sub-abort middle ground this issue adds.
    for i in 0..15 {
        std::fs::remove_file(lib.join(format!("book-{i}.epub"))).unwrap();
    }

    let w = make_worker_default(pool);
    let id = w.post(Task::Scan { library_path });
    match w.await_completion(id).await {
        TaskOutcome::Ok(detail) => {
            assert_eq!(
                detail,
                Some(TaskSuccessDetail::GhostFiles(GhostFilesWarning {
                    removed: 15,
                    total: 100,
                }))
            );
        }
        other => panic!("expected Ok with a ghost warning, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// A successful `Task::Scan` posts a follow-up `Task::BackfillThumbs`
/// (mirroring `Task::BackfillWordCounts` / `Task::BackfillPageCounts`), so by
/// the time the scan and its follow-ups have drained, every covered book's
/// three thumbnail sizes already exist on disk without anyone having loaded
/// a page — AC1, and the precondition for AC2 (the landing grid's first
/// post-scan load hits the cache instead of `thumb_cache_miss_response`'s
/// lazy path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_posts_backfill_thumbs_that_pregenerates_all_sizes_for_a_covered_book() {
    let thumbs_dir = tempfile::tempdir().unwrap();
    let covers_dir = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.path().as_os_str()))
        .also_set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.path().as_os_str()));

    let lib = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib.path());
    let library_path = lib.path().to_str().unwrap().to_string();

    let w = make_worker_default(pool().await);
    w.post(Task::Scan {
        library_path: library_path.clone(),
    });

    assert!(
        poll_maps_empty(&w).await,
        "scan and its follow-up backfills did not drain in time"
    );

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE has_cover = 1")
        .fetch_one(&w.pool)
        .await
        .expect("alpha.epub has an embedded cover, so the scan sets has_cover = 1");

    for size in crate::thumbs::ThumbSize::all() {
        let path = crate::thumbs::thumb_path_for(book_id, size);
        assert!(
            path.exists(),
            "expected a pre-generated {size} thumbnail at {path:?}"
        );
    }
}

/// [`crate::indexer::backfill_thumbs`] (via `Task::BackfillThumbs`) skips a
/// book whose three thumbnail sizes are already fresher than its
/// `last_modified` — the already-caught-up case a re-scan of an unchanged
/// library hits on every book (AC3). Seeds fresh sentinel thumbnail bytes
/// that a real encode would never produce, so any re-encoding shows up as a
/// changed file rather than relying on mtime granularity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_backfill_thumbs_skips_a_book_whose_thumbnails_are_already_fresh() {
    let thumbs_dir = tempfile::tempdir().unwrap();
    let covers_dir = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.path().as_os_str()))
        .also_set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.path().as_os_str()));

    let pool = pool().await;
    let library_path = "/lib-fresh-thumbs".to_string();
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib') RETURNING id",
    )
    .bind(&library_path)
    .fetch_one(&pool)
    .await
    .unwrap();
    // `last_modified` far in the past so any thumbnail written just now is
    // unambiguously fresher than it, regardless of clock resolution.
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, has_cover, last_modified) \
         VALUES ('uuid-fresh', 'fresh.epub', ?, '', 'Fresh', 'Fresh', 1, 1) RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // A real cover file must exist so a would-be re-encode has bytes to work
    // from — the skip is the assertion under test, not a missing-cover no-op.
    std::fs::write(
        crate::covers_dir().join("uuid-fresh.png"),
        crate::ebook::test_support::solid_color_png(200, 40, 40, 8, 8),
    )
    .unwrap();

    // Sentinel bytes no real WebP encode would produce, at each of the three
    // paths `backfill_thumbs` would touch if it decided to re-encode.
    let sentinel = b"not-a-real-webp-sentinel".to_vec();
    for size in crate::thumbs::ThumbSize::all() {
        std::fs::write(crate::thumbs::thumb_path_for(book_id, size), &sentinel).unwrap();
    }

    let w = make_worker_default(pool);
    let id = w.post(Task::BackfillThumbs { library_path });
    match w.await_completion(id).await {
        TaskOutcome::Ok(None) => {}
        other => panic!("expected Ok(None), got {other:?}"),
    }

    for size in crate::thumbs::ThumbSize::all() {
        let on_disk = std::fs::read(crate::thumbs::thumb_path_for(book_id, size)).unwrap();
        assert_eq!(
            on_disk, sentinel,
            "already-fresh thumbnail for size {size} was re-encoded"
        );
    }
}

/// AC1/AC3: `Task::DetectCleanup` serializes against concurrent cleanup runs
/// via a fixed `cleanup` resource key, and does not contend with the scan
/// semaphore.
#[test]
fn task_detect_cleanup_uses_a_fixed_resource_key_and_skips_the_scan_semaphore() {
    for kind in [
        None,
        Some(omnibus_shared::CleanupKind::Author),
        Some(omnibus_shared::CleanupKind::BookTitle),
    ] {
        let task = Task::DetectCleanup { kind };
        assert_eq!(task.resource_key(), Some("cleanup".to_string()));
        assert!(!task.uses_scan_sem());
    }
}

/// Seed a book whose title carries the `"Last, First - "` filename-cruft
/// prefix `cleanup::detect_book_titles` strips (Tier 0), against a
/// `scan_roots` row it belongs to.
async fn seed_cruft_titled_book(pool: &SqlitePool, scan_key: &str, uuid: &str, title: &str) {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, 'cleanup-lib') RETURNING id",
    )
    .bind(format!("/{scan_key}"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort) \
         VALUES (?, ?, ?, '', ?, ?)",
    )
    .bind(uuid)
    .bind(scan_key)
    .bind(lib_id)
    .bind(title)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
}

/// AC1: `Task::DetectCleanup` dispatches into the detection module and
/// persists what it finds as a `dedup_suggestions` row (migration `0069`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_detect_cleanup_persists_a_book_title_suggestion_into_dedup_suggestions() {
    let pool = pool().await;
    seed_cruft_titled_book(
        &pool,
        "cruft.epub",
        "uuid-cruft",
        "Maas, Sarah J - Throne of Glass",
    )
    .await;

    let w = make_worker_default(pool.clone());
    let id = w.post(Task::DetectCleanup { kind: None });
    match w.await_completion(id).await {
        TaskOutcome::Ok(_) => {}
        other => panic!("expected Ok, got {other:?}"),
    }

    let (kind, action): (String, String) =
        sqlx::query_as("SELECT kind, action FROM dedup_suggestions WHERE kind = 'book_title'")
            .fetch_one(&pool)
            .await
            .expect("expected a persisted book_title suggestion");
    assert_eq!(kind, "book_title");
    assert_eq!(action, "rename");
}

/// Re-running detection over an unchanged library inserts nothing new — the
/// `dedup_suggestions` table's `UNIQUE (kind, action, payload_json)`
/// constraint (migration `0069`) makes the `INSERT OR IGNORE` idempotent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_detect_cleanup_does_not_duplicate_a_suggestion_on_a_second_run() {
    let pool = pool().await;
    seed_cruft_titled_book(
        &pool,
        "cruft-again.epub",
        "uuid-cruft-again",
        "Maas, Sarah J - Crown of Midnight",
    )
    .await;

    let w = make_worker_default(pool.clone());
    for _ in 0..2 {
        let id = w.post(Task::DetectCleanup { kind: None });
        match w.await_completion(id).await {
            TaskOutcome::Ok(_) => {}
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dedup_suggestions WHERE kind = 'book_title' AND action = 'rename'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "second detection run should not duplicate the row"
    );
}

/// AC2: a successful `Task::Scan` (the worker's `indexer::reindex` path)
/// posts a follow-up `Task::DetectCleanup`, so a pre-existing dedup
/// opportunity elsewhere in the library is refreshed into
/// `dedup_suggestions` without any admin action. Detection reads the whole
/// `books` table rather than just the rows the scan itself touched, which
/// is what lets this test prove the *scan* triggered detection rather than
/// asserting on a side effect the scan produced directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_posts_detect_cleanup_that_refreshes_dedup_suggestions() {
    let thumbs_dir = tempfile::tempdir().unwrap();
    let covers_dir = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.path().as_os_str()))
        .also_set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.path().as_os_str()));

    let pool = pool().await;
    seed_cruft_titled_book(
        &pool,
        "cruft-elsewhere.epub",
        "uuid-cruft-elsewhere",
        "Maas, Sarah J - Heir of Fire",
    )
    .await;

    let lib = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib.path());
    let library_path = lib.path().to_str().unwrap().to_string();

    let w = make_worker_default(pool.clone());
    w.post(Task::Scan { library_path });

    assert!(
        poll_maps_empty(&w).await,
        "scan and its follow-up tasks (including DetectCleanup) did not drain in time"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dedup_suggestions WHERE kind = 'book_title' AND action = 'rename'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "expected the scan's DetectCleanup follow-up to persist the pre-existing suggestion"
    );
}

/// The freshness contract for `db::stats::library`'s cache. The TTL is
/// documented as the backstop, so this predicate — not the clock — is what
/// stops a just-finished scan reporting the size the reader had before it.
#[test]
fn resizes_library_selects_the_tasks_that_change_the_library_itself() {
    let path = || "/lib".to_string();
    for (name, task) in [
        (
            "Scan",
            Task::Scan {
                library_path: path(),
            },
        ),
        (
            "ScanAudiobooks",
            Task::ScanAudiobooks {
                library_path: path(),
            },
        ),
        (
            "BackfillWordCounts",
            Task::BackfillWordCounts {
                library_path: path(),
            },
        ),
        (
            "BackfillPageCounts",
            Task::BackfillPageCounts {
                library_path: path(),
            },
        ),
    ] {
        assert!(
            super::super::handlers::resizes_library(&task),
            "{name} changes how big the library is and must drop the cache"
        );
    }

    // The adjacent `Backfill*` tasks are the trap: they touch books without
    // moving any figure the card reports, so paying for a recompute after one
    // would be waste, not freshness.
    for (name, task) in [
        (
            "BackfillChapters",
            Task::BackfillChapters {
                library_path: path(),
            },
        ),
        (
            "BackfillThumbs",
            Task::BackfillThumbs {
                library_path: path(),
            },
        ),
        ("RebuildFtsIndex", Task::RebuildFtsIndex),
        ("RefetchAuthorPhotos", Task::RefetchAuthorPhotos),
        ("KepubConvert", Task::KepubConvert { book_id: 1 }),
    ] {
        assert!(
            !super::super::handlers::resizes_library(&task),
            "{name} leaves the library's size alone and must keep the cache"
        );
    }
}
