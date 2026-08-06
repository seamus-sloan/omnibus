//! Unit tests for the Web→Kobo annotation materialization: the happy
//! derivation path against real on-disk source/kepub fixtures (including
//! client-id minting), the degrade-to-unresolved paths (missing kepub
//! cache, underivable CFI), the conversion request that makes a cold cache
//! recoverable, and the whole-backlog boot pass.

use omnibus_shared::{CreateHighlight, EbookMetadata, HighlightColor};
use sqlx::SqlitePool;

use super::*;
use crate::annotations::{
    create_highlight, ingest_kobo_annotations, served_kobo_annotations, IngestKoboAnnotation,
};
use crate::init_db;
use crate::kobo_position::parse_annotation_location;
use crate::test_support::{build_test_epub, build_test_kepub, make_test_dir, EnvVarGuard};
use crate::worker::{Worker, WorkerConfig};

const SOURCE_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body>
  <p>First sentence here. Second sentence follows.</p>
</body>
</html>"#;

const KEPUB_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body>
  <div id="book-columns"><div id="book-inner">
  <p><span class="koboSpan" id="kobo.1.1">First sentence here. </span><span class="koboSpan" id="kobo.1.2">Second sentence follows.</span></p>
  </div></div>
</body>
</html>"#;

/// The web-reader highlight covering "Second sentence follows." — derives
/// to all of `kobo.1.2`.
const WEB_CFI: &str = "epubcfi(/6/2!/4/2,/1:21,/1:45)";

/// Seed one book whose EPUB really exists on disk in a per-test library
/// dir, plus a user; returns `(pool, user_id, book_id, book_uuid, dir)`.
async fn fixture(tag: &str) -> (SqlitePool, i64, i64, String, std::path::PathBuf) {
    let dir = make_test_dir(&format!("annotation_downsync_{tag}"));
    std::fs::write(
        dir.join("book.epub"),
        build_test_epub(&[("c1.xhtml", SOURCE_C1)]),
    )
    .unwrap();

    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::replace_books(
        &pool,
        dir.to_str().unwrap(),
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: "book.epub".into(),
                title: Some("Book".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();
    let books = crate::list_books(&pool, dir.to_str().unwrap())
        .await
        .unwrap();
    let book = books.first().unwrap();
    let uuid = book.unique_identifier.clone().unwrap();
    let user = seed_user(&pool, "alice").await;
    (pool, user, book.id, uuid, dir)
}

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Write the book's kepub into a per-test cache dir and point
/// `OMNIBUS_KEPUB_DIR` at it for the test's lifetime.
fn kepub_cache(dir: &std::path::Path, book_id: i64) -> EnvVarGuard {
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{book_id}.kepub.epub")),
        build_test_kepub(&[("c1.xhtml", KEPUB_C1)]),
    )
    .unwrap();
    EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()))
}

fn web_highlight(uuid: &str, cfi: &str, client_id: Option<&str>) -> CreateHighlight {
    CreateHighlight {
        book_uuid: uuid.to_string(),
        epub_cfi_range: cfi.to_string(),
        color: HighlightColor::Amber,
        text: Some("Second sentence follows.".into()),
        client_id: client_id.map(str::to_owned),
    }
}

#[tokio::test]
async fn downsync_book_annotations_derives_locations_and_mints_client_ids() {
    let (pool, user, book_id, uuid, dir) = fixture("happy").await;
    let _guard = kepub_cache(&dir, book_id);
    create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, None))
        .await
        .unwrap();
    let before: (Option<String>, i64) =
        sqlx::query_as("SELECT client_id, updated_at FROM annotations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before.0, None);

    let stats = downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .unwrap();

    assert_eq!(
        stats,
        DownsyncStats {
            derived: 1,
            unresolved: 0
        }
    );
    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served.len(), 1);
    let range = parse_annotation_location(&served[0].kobo_location).expect("kobospan range");
    assert_eq!(range.source, "c1.xhtml");
    assert_eq!(
        (range.start.n, range.start.m, range.start.char_utf16),
        (1, 2, 0)
    );
    assert_eq!((range.end.n, range.end.m, range.end.char_utf16), (1, 2, 24));
    // A minted id makes the served identity stable; the untouched edit time
    // keeps the wire fingerprint honest (membership is what re-reports).
    let after: (Option<String>, i64) =
        sqlx::query_as("SELECT client_id, updated_at FROM annotations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(after.0.is_some());
    assert_eq!(after.1, before.1);
}

#[tokio::test]
async fn downsync_book_annotations_keeps_an_existing_client_id() {
    let (pool, user, book_id, uuid, dir) = fixture("keepid").await;
    let _guard = kepub_cache(&dir, book_id);
    create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, Some("web-1")))
        .await
        .unwrap();

    downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .unwrap();

    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served[0].client_id, "web-1");
}

#[tokio::test]
async fn downsync_book_annotations_leaves_rows_unresolved_without_a_kepub_cache() {
    let (pool, user, _book_id, uuid, dir) = fixture("nokepub").await;
    // Point the cache at an empty dir: nothing to derive spans from.
    let kepub_dir = dir.join("kepub-empty");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));
    create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, None))
        .await
        .unwrap();

    let stats = downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .unwrap();

    assert_eq!(
        stats,
        DownsyncStats {
            derived: 0,
            unresolved: 1
        }
    );
    assert!(served_kobo_annotations(&pool, user, &uuid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn downsync_book_annotations_requests_a_kepub_conversion_when_the_cache_is_cold() {
    let (pool, user, book_id, uuid, dir) = fixture("coldcache").await;
    let kepub_dir = dir.join("kepub-empty");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));
    create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, None))
        .await
        .unwrap();
    let worker = Worker::new(pool.clone(), WorkerConfig::default());

    let stats = downsync_book_annotations(&pool, Some(&worker), user, &uuid)
        .await
        .unwrap();

    // Still nothing derivable this pass — but without the request the row
    // would stay unresolved forever, since nothing else on this path warms
    // the cache and an unmaterialized row never moves the wire fingerprint.
    assert_eq!(stats.derived, 0);
    let snapshot = worker.progress_snapshot();
    assert!(
        snapshot
            .active
            .iter()
            .chain(&snapshot.recent_complete)
            .any(|t| t.resource_key.as_deref() == Some(&format!("kepub:{book_id}"))),
        "expected a KepubConvert request for book {book_id}, got {snapshot:?}"
    );
}

#[tokio::test]
async fn downsync_book_id_annotations_materializes_every_users_rows_for_one_book() {
    let (pool, alice, book_id, uuid, dir) = fixture("byid").await;
    let _guard = kepub_cache(&dir, book_id);
    let bob = seed_user(&pool, "bob").await;
    for user in [alice, bob] {
        create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, None))
            .await
            .unwrap();
    }

    let stats = downsync_book_id_annotations(&pool, book_id).await.unwrap();

    assert_eq!(
        stats,
        DownsyncStats {
            derived: 2,
            unresolved: 0
        }
    );
    for user in [alice, bob] {
        assert_eq!(
            served_kobo_annotations(&pool, user, &uuid)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn downsync_book_id_annotations_noops_for_an_unknown_book_id() {
    let (pool, _user, _book_id, _uuid, _dir) = fixture("byid_missing").await;

    let stats = downsync_book_id_annotations(&pool, 9_999).await.unwrap();

    assert_eq!(stats, DownsyncStats::default());
}

#[tokio::test]
async fn downsync_book_annotations_counts_underivable_cfis_as_unresolved() {
    let (pool, user, book_id, uuid, dir) = fixture("badcfi").await;
    let _guard = kepub_cache(&dir, book_id);
    // A point CFI is not a highlight range; it must degrade, never guess.
    create_highlight(
        &pool,
        user,
        &web_highlight(&uuid, "epubcfi(/6/2!/4/2/1:0)", None),
    )
    .await
    .unwrap();

    let stats = downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .unwrap();

    assert_eq!(
        stats,
        DownsyncStats {
            derived: 0,
            unresolved: 1
        }
    );
}

#[tokio::test]
async fn downsync_book_annotations_noops_when_nothing_is_pending() {
    let (pool, user, book_id, uuid, dir) = fixture("noop").await;
    let _guard = kepub_cache(&dir, book_id);
    // A device-origin row already carries its anchor — not a candidate.
    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[IngestKoboAnnotation {
            client_id: "kobo-1".into(),
            color: HighlightColor::Amber,
            text: None,
            note: None,
            kobo_location: r#"{"span":{"chapterFilename":"c1.xhtml","startPath":"span#kobo\\.1\\.1","startChar":0,"endPath":"span#kobo\\.1\\.1","endChar":5}}"#.into(),
            epub_cfi_range: None,
        }],
        &[],
    )
    .await
    .unwrap();

    let stats = downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .unwrap();

    assert_eq!(stats, DownsyncStats::default());
}

#[tokio::test]
async fn downsync_all_kobo_annotations_converts_every_users_pending_rows() {
    let (pool, alice, book_id, uuid, dir) = fixture("bootpass").await;
    let _guard = kepub_cache(&dir, book_id);
    let bob = seed_user(&pool, "bob").await;
    for user in [alice, bob] {
        create_highlight(&pool, user, &web_highlight(&uuid, WEB_CFI, None))
            .await
            .unwrap();
    }

    let stats = downsync_all_kobo_annotations(&pool, None).await.unwrap();

    assert_eq!(
        stats,
        DownsyncStats {
            derived: 2,
            unresolved: 0
        }
    );
    for user in [alice, bob] {
        assert_eq!(
            served_kobo_annotations(&pool, user, &uuid)
                .await
                .unwrap()
                .len(),
            1
        );
    }
    // Caught up: a second pass finds nothing pending.
    assert_eq!(
        downsync_all_kobo_annotations(&pool, None).await.unwrap(),
        DownsyncStats::default()
    );
}

#[tokio::test]
async fn downsync_all_kobo_annotations_continues_past_a_failing_book_and_reports_the_rest() {
    let (pool, alice, book_id, uuid, dir) = fixture("bootfail_good").await;
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{book_id}.kepub.epub")),
        build_test_kepub(&[("c1.xhtml", KEPUB_C1)]),
    )
    .unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_KEPUB_DIR", Some(kepub_dir.as_os_str()));
    create_highlight(&pool, alice, &web_highlight(&uuid, WEB_CFI, None))
        .await
        .unwrap();

    // A second book whose kepub cache file exists but isn't a valid zip:
    // `annotation_locations` errors opening it, so its
    // `downsync_book_annotations` call returns `Err` and must not abort the
    // pass before the first (good) book is processed.
    let bad_dir = make_test_dir("annotation_downsync_bootfail_bad");
    std::fs::write(
        bad_dir.join("bad.epub"),
        build_test_epub(&[("c1.xhtml", SOURCE_C1)]),
    )
    .unwrap();
    crate::replace_books(
        &pool,
        bad_dir.to_str().unwrap(),
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: "bad.epub".into(),
                title: Some("Bad Book".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();
    let bad_book = crate::list_books(&pool, bad_dir.to_str().unwrap())
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let bad_uuid = bad_book.unique_identifier.clone().unwrap();
    std::fs::write(
        kepub_dir.join(format!("{}.kepub.epub", bad_book.id)),
        b"not a zip file",
    )
    .unwrap();
    create_highlight(&pool, alice, &web_highlight(&bad_uuid, WEB_CFI, None))
        .await
        .unwrap();

    let stats = downsync_all_kobo_annotations(&pool, None).await.unwrap();

    // Only the good book's row is counted — the failing book's error was
    // caught and logged, not folded into the pass's stats.
    assert_eq!(
        stats,
        DownsyncStats {
            derived: 1,
            unresolved: 0
        }
    );
    assert_eq!(
        served_kobo_annotations(&pool, alice, &uuid)
            .await
            .unwrap()
            .len(),
        1
    );
    // The failing book's row is untouched — proof the loop reached it and
    // moved on, rather than the pass having stopped before it.
    let (kobo_location,): (Option<String>,) =
        sqlx::query_as("SELECT kobo_location FROM annotations WHERE user_id = ? AND book_uuid = ?")
            .bind(alice)
            .bind(&bad_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(kobo_location.is_none());
}

#[tokio::test]
async fn downsync_book_annotations_propagates_db_error_when_pool_is_closed() {
    let (pool, user, _book_id, uuid, _dir) = fixture("dberr").await;
    pool.close().await;
    assert!(downsync_book_annotations(&pool, None, user, &uuid)
        .await
        .is_err());
}

#[tokio::test]
async fn downsync_book_id_annotations_propagates_db_error_when_pool_is_closed() {
    let (pool, _user, book_id, _uuid, _dir) = fixture("byid_dberr").await;
    pool.close().await;
    assert!(downsync_book_id_annotations(&pool, book_id).await.is_err());
}
