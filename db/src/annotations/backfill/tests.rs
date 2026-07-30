//! Unit tests for the boot-time Kobo annotation CFI backfill: the happy
//! derivation path against real on-disk source/kepub fixtures, and the
//! degrade-to-unresolved paths (missing kepub cache, unparseable anchor).

use omnibus_shared::{EbookMetadata, HighlightColor};
use sqlx::SqlitePool;

use super::*;
use crate::annotations::{ingest_kobo_annotations, list_highlights, IngestKoboAnnotation};
use crate::init_db;
use crate::test_support::{build_test_epub, build_test_kepub, make_test_dir, EnvVarGuard};

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

/// Anchor covering all of `kobo.1.2` ("Second sentence follows.").
const LOCATION: &str = r#"{"span":{"chapterFilename":"c1.xhtml","startPath":"span#kobo\\.1\\.2","startChar":0,"endPath":"span#kobo\\.1\\.2","endChar":24}}"#;

/// Seed one book whose EPUB really exists on disk in a per-test library
/// dir, plus a user; returns `(pool, user_id, book_id, book_uuid, dir)`.
async fn fixture(tag: &str) -> (SqlitePool, i64, i64, String, std::path::PathBuf) {
    let dir = make_test_dir(&format!("annotation_backfill_{tag}"));
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
    let user = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES ('alice', '!x', 0, 0, 0, 1) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    (pool, user, book.id, uuid, dir)
}

fn upload(location: &str) -> IngestKoboAnnotation {
    IngestKoboAnnotation {
        client_id: "kobo-1".into(),
        color: HighlightColor::Amber,
        text: Some("Second sentence follows.".into()),
        note: None,
        kobo_location: location.into(),
        epub_cfi_range: None,
    }
}

#[tokio::test]
async fn backfill_kobo_annotation_cfis_derives_ranges_for_rows_missing_them() {
    let (pool, user, book_id, uuid, dir) = fixture("happy").await;
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{book_id}.kepub.epub")),
        build_test_kepub(&[("c1.xhtml", KEPUB_C1)]),
    )
    .unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));

    ingest_kobo_annotations(&pool, user, &uuid, &[upload(LOCATION)], &[])
        .await
        .unwrap();
    let stats = backfill_kobo_annotation_cfis(&pool).await.unwrap();

    assert_eq!(
        stats,
        BackfillStats {
            derived: 1,
            unresolved: 0
        }
    );
    let rows = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        rows[0].epub_cfi_range.as_deref(),
        Some("epubcfi(/6/2!/4/2,/1:21,/1:45)")
    );
}

#[tokio::test]
async fn backfill_kobo_annotation_cfis_leaves_rows_unresolved_without_a_kepub_cache() {
    let (pool, user, _book_id, uuid, dir) = fixture("nokepub").await;
    // Point the cache at an empty dir: nothing to derive from.
    let kepub_dir = dir.join("kepub-empty");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));

    ingest_kobo_annotations(&pool, user, &uuid, &[upload(LOCATION)], &[])
        .await
        .unwrap();
    let stats = backfill_kobo_annotation_cfis(&pool).await.unwrap();

    assert_eq!(
        stats,
        BackfillStats {
            derived: 0,
            unresolved: 1
        }
    );
    let rows = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(rows[0].epub_cfi_range, None);
}

#[tokio::test]
async fn backfill_kobo_annotation_cfis_counts_unparseable_anchors_as_unresolved() {
    let (pool, user, book_id, uuid, dir) = fixture("badloc").await;
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{book_id}.kepub.epub")),
        build_test_kepub(&[("c1.xhtml", KEPUB_C1)]),
    )
    .unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));

    ingest_kobo_annotations(&pool, user, &uuid, &[upload(r#"{"opaque":true}"#)], &[])
        .await
        .unwrap();
    let stats = backfill_kobo_annotation_cfis(&pool).await.unwrap();

    assert_eq!(
        stats,
        BackfillStats {
            derived: 0,
            unresolved: 1
        }
    );
}
