//! Drives the whole cold-cache recovery loop through a **real** kepubify
//! conversion: a web highlight written while no KEPUB is cached must end up
//! Kobo-placeable on its own, without another write or a restart. The unit
//! fixtures can only assert the two halves separately (the conversion is
//! requested; a warm cache materializes) — this is the seam where they meet.
//! Skips (passes trivially) when the binary is absent, so the slim-shell
//! matrix stays green; run inside `nix develop .#web` to exercise it.

use std::time::{Duration, Instant};

use omnibus_db::annotations::{
    create_highlight, downsync_book_annotations, served_kobo_annotations,
};
use omnibus_db::kobo_position::parse_annotation_location;
use omnibus_db::test_support::{build_test_epub, make_test_dir, EnvVarGuard};
use omnibus_db::worker::{Worker, WorkerConfig};
use omnibus_shared::{CreateHighlight, EbookMetadata, HighlightColor};

const CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body>
  <p>First sentence here. Second sentence follows.</p>
</body>
</html>"#;

/// The web-reader highlight covering "Second sentence follows." — a range
/// against the *source* EPUB's structure, which is what the reader emits.
const WEB_CFI: &str = "epubcfi(/6/2!/4/2,/1:21,/1:45)";

/// How long to wait for the worker to convert and re-run the pass. Generous:
/// a miss here is a failed assertion, so it must not be a flake budget.
const SETTLE: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_web_highlight_reaches_kobo_after_the_conversion_it_requested() {
    if !omnibus_db::kepubify_available() {
        eprintln!("kepubify unavailable; skipping cold-cache downsync loop");
        return;
    }
    let dir = make_test_dir("kobo_downsync_cold_cache");
    std::fs::write(
        dir.join("book.epub"),
        build_test_epub(&[("c1.xhtml", CHAPTER)]),
    )
    .unwrap();
    // An empty cache dir is the whole precondition: nothing to anchor against.
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));

    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    omnibus_db::replace_books(
        &pool,
        dir.to_str().unwrap(),
        vec![omnibus_db::ebook::IndexedBook {
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
    let books = omnibus_db::list_books(&pool, dir.to_str().unwrap())
        .await
        .unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES ('alice', '!x', 0, 0, 0, 1) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    create_highlight(
        &pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: WEB_CFI.into(),
            color: HighlightColor::Amber,
            text: Some("Second sentence follows.".into()),
            client_id: None,
        },
    )
    .await
    .unwrap();

    let worker = Worker::new(pool.clone(), WorkerConfig::default());
    let stats = downsync_book_annotations(&pool, Some(&worker), user, &uuid)
        .await
        .unwrap();
    // Nothing derivable yet — the point is that this pass leaves work queued
    // rather than abandoning the row.
    assert_eq!(stats.derived, 0);
    assert!(
        served_kobo_annotations(&pool, user, &uuid)
            .await
            .unwrap()
            .is_empty(),
        "nothing should be servable before the conversion lands"
    );

    let deadline = Instant::now() + SETTLE;
    let served = loop {
        let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
        if !served.is_empty() {
            break served;
        }
        assert!(
            Instant::now() < deadline,
            "highlight never materialized: the requested conversion did not \
             re-run the down-sync"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(served.len(), 1);
    assert!(
        std::path::Path::new(&kepub_dir)
            .join(format!("{}.kepub.epub", books[0].id))
            .exists(),
        "the conversion should have been performed, not just requested"
    );
    let range = parse_annotation_location(&served[0].kobo_location).expect("kobospan range");
    assert_eq!(range.source, "c1.xhtml");
    // Real kepubify segmentation, not the hand-written fixture's: assert the
    // anchor is a non-empty forward range rather than pinning span numbers.
    assert!(
        (range.start.n, range.start.m, range.start.char_utf16)
            < (range.end.n, range.end.m, range.end.char_utf16)
    );
}
