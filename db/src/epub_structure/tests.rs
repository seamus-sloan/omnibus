//! Tests for the structure tables: replace/read round-trip and idempotency,
//! percent arithmetic parity rules, and the post-scan backfill end to end
//! (extracts, converges, and never fabricates chapters).

use omnibus_shared::EbookMetadata;
use sqlx::SqlitePool;

use crate::ebook::toc::{EpubStructure, SpineStat, TocChapter};
use crate::init_db;
use crate::test_support::{
    build_test_epub, build_test_epub_with_nav, make_test_dir, seed_minimal_books,
};

use super::*;

fn sample_structure() -> EpubStructure {
    EpubStructure {
        spine: vec![
            SpineStat {
                spine_index: 0,
                href: "c1.xhtml".into(),
                visible_chars: 40,
            },
            SpineStat {
                spine_index: 1,
                href: "c2.xhtml".into(),
                visible_chars: 60,
            },
        ],
        chapters: vec![TocChapter {
            ordinal: 0,
            title: "Chapter Two".into(),
            href: "c2.xhtml".into(),
            spine_index: 1,
            start_chars: 40,
        }],
    }
}

async fn seeded_file_id(pool: &SqlitePool) -> i64 {
    seed_minimal_books(pool, 1).await;
    sqlx::query_scalar("SELECT id FROM book_files LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn replace_structure_round_trips_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let file_id = seeded_file_id(&pool).await;

    replace_structure(&pool, file_id, &sample_structure())
        .await
        .unwrap();
    replace_structure(&pool, file_id, &sample_structure())
        .await
        .unwrap();

    let stats = get_spine_stats(&pool, file_id).await.unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!((stats[0].visible_chars, stats[0].chars_before), (40, 0));
    assert_eq!((stats[1].visible_chars, stats[1].chars_before), (60, 40));

    let chapters = get_chapters(&pool, file_id).await.unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title, "Chapter Two");
    assert_eq!(chapters[0].start_chars, 40);
}

#[tokio::test]
async fn structure_rows_cascade_away_with_their_book_file() {
    // The Changed sync path deletes + re-inserts `book_files`; the cascade
    // is what invalidates stale structure, so prove it fires.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let file_id = seeded_file_id(&pool).await;
    replace_structure(&pool, file_id, &sample_structure())
        .await
        .unwrap();

    sqlx::query("DELETE FROM book_files WHERE id = ?")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(get_spine_stats(&pool, file_id).await.unwrap().is_empty());
    assert!(get_chapters(&pool, file_id).await.unwrap().is_empty());
}

#[test]
fn percent_at_uses_floor_semantics_and_clamps() {
    let stats = vec![
        SpineStatRow {
            spine_index: 0,
            href: "c1.xhtml".into(),
            visible_chars: 40,
            chars_before: 0,
        },
        SpineStatRow {
            spine_index: 1,
            href: "c2.xhtml".into(),
            visible_chars: 60,
            chars_before: 40,
        },
    ];
    assert_eq!(percent_at(&stats, 0, 0), Some(0));
    assert_eq!(percent_at(&stats, 1, 0), Some(40));
    assert_eq!(percent_at(&stats, 1, 30), Some(70));
    // Offsets past the end clamp rather than exceed 100.
    assert_eq!(percent_at(&stats, 1, 10_000), Some(100));
    // A spine index the stats don't cover is a refusal, not a guess.
    assert_eq!(percent_at(&stats, 7, 0), None);
    assert_eq!(percent_at(&[], 0, 0), None);
}

#[test]
fn percent_at_reports_zero_for_an_empty_book() {
    let stats = vec![SpineStatRow {
        spine_index: 0,
        href: "c1.xhtml".into(),
        visible_chars: 0,
        chars_before: 0,
    }];
    assert_eq!(percent_at(&stats, 0, 0), Some(0));
}

/// Seed one on-disk EPUB through `replace_books` and return its
/// `book_files.id` plus the library dir.
async fn seed_epub_book(pool: &SqlitePool, tag: &str, bytes: Vec<u8>) -> (i64, String) {
    let dir = make_test_dir(&format!("epub_structure_{tag}"));
    std::fs::write(dir.join("book.epub"), bytes).unwrap();
    crate::replace_books(
        pool,
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
    let file_id = sqlx::query_scalar("SELECT id FROM book_files LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    (file_id, dir.to_str().unwrap().to_string())
}

const CH: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C</title></head>
<body><p>First sentence here. Second sentence follows.</p></body>
</html>"#;

#[tokio::test]
async fn backfill_epub_structure_extracts_and_converges() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let epub = build_test_epub_with_nav(
        &[("c1.xhtml", CH), ("c2.xhtml", CH)],
        &[("One", "c1.xhtml"), ("Two", "c2.xhtml")],
    );
    let (file_id, dir) = seed_epub_book(&pool, "happy", epub).await;

    let mut seen = 0u32;
    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| seen += 1)
        .await
        .unwrap();
    assert_eq!(seen, 1);
    assert_eq!(get_spine_stats(&pool, file_id).await.unwrap().len(), 2);
    assert_eq!(get_chapters(&pool, file_id).await.unwrap().len(), 2);

    // Converged: the candidate query keys on existing stats, so a second
    // run visits nothing.
    let mut second = 0u32;
    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| second += 1)
        .await
        .unwrap();
    assert_eq!(second, 0);
}

#[tokio::test]
async fn backfill_epub_structure_marks_a_tocless_book_done_with_zero_chapters() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let epub = build_test_epub(&[("c1.xhtml", CH)]);
    let (file_id, dir) = seed_epub_book(&pool, "tocless", epub).await;

    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| {})
        .await
        .unwrap();
    assert!(!get_spine_stats(&pool, file_id).await.unwrap().is_empty());
    assert!(get_chapters(&pool, file_id).await.unwrap().is_empty());

    // "Extracted, honestly TOC-less" is done — not retried forever.
    let mut retries = 0u32;
    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| retries += 1)
        .await
        .unwrap();
    assert_eq!(retries, 0);
}

#[tokio::test]
async fn backfill_epub_structure_skips_an_unreadable_file_and_retries_later() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // The book_files row exists but the file on disk is garbage.
    let (file_id, dir) = seed_epub_book(&pool, "garbage", b"not an epub".to_vec()).await;

    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| {})
        .await
        .unwrap();
    assert!(get_spine_stats(&pool, file_id).await.unwrap().is_empty());

    // Still a candidate next time — unreadable is retried, not tombstoned.
    let mut seen = 0u32;
    crate::indexer::backfill_epub_structure(&pool, &dir, |_, _, _| seen += 1)
        .await
        .unwrap();
    assert_eq!(seen, 1);
}
