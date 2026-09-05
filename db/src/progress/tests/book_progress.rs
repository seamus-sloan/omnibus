//! The per-book progress read: every format at once, which one is furthest,
//! and the resolved chapter block each record carries.

use omnibus_shared::{PositionConfidence, ProgressUpdate};
use sqlx::SqlitePool;

use crate::init_db;

use super::super::*;
use super::{seed, seed_audiobook, seed_named_file, seed_user};

/// Retitle an audiobook's chapter marks. `seed_audiobook` writes three real
/// ones; passing `"Part {n}"` shapes makes them the indexer's synthetic
/// per-part fallback instead.
async fn retitle_chapters(pool: &SqlitePool, book_id: i64, titles: &[&str]) {
    let file_id: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? ORDER BY id LIMIT 1")
            .bind(book_id)
            .fetch_one(pool)
            .await
            .unwrap();
    for (i, title) in titles.iter().enumerate() {
        sqlx::query("UPDATE file_chapters SET title = ? WHERE book_file_id = ? AND ordinal = ?")
            .bind(title)
            .bind(file_id)
            .bind(i as i64 + 1)
            .execute(pool)
            .await
            .unwrap();
    }
}

/// Give a book's EPUB file the derived structure tables the resolver reads,
/// without needing a real archive on disk: four equal spine documents and a
/// three-entry TOC that skips the first (front matter outside the TOC).
async fn seed_epub_structure(pool: &SqlitePool, book_id: i64) {
    let file_id: i64 = sqlx::query_scalar(
        "SELECT id FROM book_files WHERE book_id = ? AND format = 'EPUB' ORDER BY id LIMIT 1",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap();
    for spine_index in 0..4i64 {
        sqlx::query(
            "INSERT INTO epub_spine_stats (book_file_id, spine_index, href, visible_chars, chars_before)
             VALUES (?, ?, ?, 1000, ?)",
        )
        .bind(file_id)
        .bind(spine_index)
        .bind(format!("c{spine_index}.xhtml"))
        .bind(spine_index * 1000)
        .execute(pool)
        .await
        .unwrap();
    }
    for (ordinal, title, spine_index) in [(0i64, "One", 1i64), (1, "Two", 2), (2, "Three", 3)] {
        sqlx::query(
            "INSERT INTO ebook_chapters (book_file_id, ordinal, title, href, spine_index, start_chars)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(title)
        .bind(format!("c{spine_index}.xhtml"))
        .bind(spine_index)
        .bind(spine_index * 1000)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Write a percent-only epub position — the shape a Kobo sends, which carries
/// no CFI to walk.
async fn save_epub_percent(pool: &SqlitePool, user: i64, uuid: &str, percent: i64, at: i64) {
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(percent),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(at),
        },
    )
    .await
    .unwrap();
}

async fn save_audio(pool: &SqlitePool, user: i64, uuid: &str, seconds: f64, at: i64) {
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(seconds),
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(at),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn book_progress_returns_every_format_and_names_the_further_one_as_furthest() {
    // The failure this endpoint shape exists to end: answering with the epub
    // row alone reported a reader most of the way through the audiobook as
    // barely started, with nothing in the payload to say so.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "dual-format";
    let book_id = seed_audiobook(&pool, uuid).await;
    sqlx::query("INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', 'a.epub', 1)")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    seed_epub_structure(&pool, book_id).await;

    // 47% read; 900 s of a 1200 s audiobook is 75% listened. The epub row is
    // written *later*, so recency alone would name the wrong one.
    save_audio(&pool, user, uuid, 900.0, 100).await;
    save_epub_percent(&pool, user, uuid, 47, 200).await;

    let progress = book_progress(&pool, user, uuid, None)
        .await
        .unwrap()
        .expect("book exists");
    assert_eq!(progress.records.len(), 2);
    assert_eq!(progress.furthest, Some(ProgressFormat::Audio));
}

#[tokio::test]
async fn book_progress_falls_back_to_recency_when_a_record_reports_no_percent() {
    // A CFI-only epub row has no percent yet (the derivation runs off-path),
    // and ranking a known 75% against an assumed zero would be a guess. The
    // most recent event time is the honest tie-break.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "no-percent";
    let book_id = seed_audiobook(&pool, uuid).await;
    sqlx::query("INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', 'a.epub', 1)")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    save_audio(&pool, user, uuid, 900.0, 100).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:5)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(200),
        },
    )
    .await
    .unwrap();

    let progress = book_progress(&pool, user, uuid, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.furthest, Some(ProgressFormat::Epub));
}

#[tokio::test]
async fn book_progress_narrows_to_one_record_when_given_a_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "narrowed";
    let book_id = seed_audiobook(&pool, uuid).await;
    sqlx::query("INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', 'a.epub', 1)")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    save_audio(&pool, user, uuid, 900.0, 100).await;
    save_epub_percent(&pool, user, uuid, 47, 200).await;

    let progress = book_progress(&pool, user, uuid, Some(ProgressFormat::Epub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.records.len(), 1);
    assert_eq!(progress.records[0].format, ProgressFormat::Epub);
    assert_eq!(progress.furthest, Some(ProgressFormat::Epub));
}

#[tokio::test]
async fn book_progress_separates_an_unknown_book_from_an_unopened_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Untouched").await;

    assert!(book_progress(&pool, user, "no-such-uuid", None)
        .await
        .unwrap()
        .is_none());
    let opened = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .expect("the book exists");
    assert!(opened.records.is_empty());
    assert_eq!(opened.furthest, None);
}

#[tokio::test]
async fn book_progress_gives_an_audio_record_a_runtime_and_a_percent() {
    // Neither is stored: the write path rejects a percent on an audio row, so
    // a caller with no runtime had to guess one from training data.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "runtime";
    seed_audiobook(&pool, uuid).await;
    save_audio(&pool, user, uuid, 900.0, 100).await;

    let progress = book_progress(&pool, user, uuid, None)
        .await
        .unwrap()
        .unwrap();
    let record = &progress.records[0];
    assert_eq!(record.total_duration_seconds, Some(1200.0));
    assert_eq!(record.progress_percent, Some(75));
}

#[tokio::test]
async fn book_progress_resolves_an_audio_position_to_its_chapter_mark() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "audio-chapter";
    let book_id = seed_audiobook(&pool, uuid).await;
    retitle_chapters(&pool, book_id, &["Openings", "The Middle", "Endings"]).await;
    // 900 s lands 100 s into the third mark (800..1200).
    save_audio(&pool, user, uuid, 900.0, 100).await;

    let progress = book_progress(&pool, user, uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0]
        .resolved
        .as_ref()
        .expect("an audiobook with marks resolves");
    assert_eq!(resolved.chapter_title.as_deref(), Some("Endings"));
    assert_eq!(resolved.chapter_ordinal, Some(3));
    assert_eq!(resolved.chapters_total, Some(3));
    assert_eq!(resolved.percent_through_chapter, Some(25));
    assert_eq!(resolved.percent_through_book, Some(75));
    assert_eq!(resolved.confidence, PositionConfidence::High);
}

#[tokio::test]
async fn book_progress_reports_low_confidence_for_synthetic_audio_marks() {
    // The 65-chapter novel stored as a handful of M4B parts: the marks are
    // real boundaries, but calling them chapters is how "part 3 of 3" got read
    // as the end of the book.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "synthetic";
    let book_id = seed_audiobook(&pool, uuid).await;
    retitle_chapters(&pool, book_id, &["Part 1", "Part 2", "Part 3"]).await;
    save_audio(&pool, user, uuid, 900.0, 100).await;

    let progress = book_progress(&pool, user, uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0].resolved.as_ref().unwrap();
    // Reported, not withheld — a caller can still say roughly where the
    // reader is, as long as it knows not to call it a chapter.
    assert_eq!(resolved.chapter_title.as_deref(), Some("Part 3"));
    assert_eq!(resolved.confidence, PositionConfidence::Low);
}

#[tokio::test]
async fn book_progress_resolves_a_percent_only_epub_position_onto_the_spine() {
    // A Kobo writes a percent and no CFI. Inverting it back onto the spine is
    // the best available answer and is coarse by construction, so it says so.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Kobo Read").await;
    seed_epub_structure(&pool, book_id).await;
    // 60% of 4000 chars is 2400 — spine document 2, which chapter "Two" owns.
    save_epub_percent(&pool, user, &uuid, 60, 100).await;

    let progress = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0].resolved.as_ref().unwrap();
    assert_eq!(resolved.spine_index, Some(2));
    assert_eq!(resolved.chapter_title.as_deref(), Some("Two"));
    assert_eq!(resolved.chapter_ordinal, Some(2));
    assert_eq!(resolved.chapters_total, Some(3));
    assert_eq!(resolved.percent_through_book, Some(60));
    assert_eq!(resolved.percent_through_chapter, Some(40));
    assert_eq!(resolved.confidence, PositionConfidence::Low);
}

#[tokio::test]
async fn book_progress_resolves_a_comic_page_to_its_percent_and_no_chapter() {
    // A comic anchor addresses a page, not a spine position — reporting a
    // chapter for one would be an invention.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Comic").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some(omnibus_shared::comic_page_anchor(3)),
            audio_position_seconds: None,
            progress_percent: Some(50),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let progress = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0].resolved.as_ref().unwrap();
    assert_eq!(resolved.percent_through_book, Some(50));
    assert_eq!(resolved.chapter_title, None);
    assert_eq!(resolved.spine_index, None);
    assert_eq!(resolved.confidence, PositionConfidence::High);
}

#[tokio::test]
async fn book_progress_leaves_the_resolved_block_absent_when_a_book_has_no_structure() {
    // Nothing to resolve against is a different answer from a coarse one, and
    // must not be dressed up as a chapter.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Unbackfilled").await;
    save_epub_percent(&pool, user, &uuid, 40, 100).await;

    let progress = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .unwrap();
    assert!(progress.records[0].resolved.is_none());
}

#[tokio::test]
async fn the_resume_feed_names_the_chapter_without_opening_the_archive() {
    // The landing rail renders up to twenty cards per load. A CFI carries its
    // own spine step, so the chapter is free — and this book's EPUB does not
    // exist on disk, which is exactly what proves nothing opened it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Feed Card").await;
    seed_epub_structure(&pool, book_id).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            // `/6/6` is the package step for spine index 2, which chapter
            // "Two" owns.
            epub_cfi: Some("epubcfi(/6/6!/4/2/1:5)".into()),
            audio_position_seconds: None,
            progress_percent: Some(55),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    let resolved = points[0].record.resolved.as_ref().expect("resolved");
    assert_eq!(resolved.spine_index, Some(2));
    assert_eq!(resolved.chapter_title.as_deref(), Some("Two"));
    assert_eq!(resolved.chapter_ordinal, Some(2));
    assert_eq!(resolved.confidence, PositionConfidence::High);
    // The stored percent is the honest whole-book figure without a walk...
    assert_eq!(resolved.percent_through_book, Some(55));
    // ...and a percentage *of the chapter* needs the in-document offset the
    // fast path deliberately doesn't fetch, so it is omitted rather than
    // reported as the chapter's start.
    assert_eq!(resolved.percent_through_chapter, None);
}

#[tokio::test]
async fn the_per_book_read_walks_the_cfi_for_a_percent_through_the_chapter() {
    // The same position asked for deliberately, one book at a time: here the
    // archive walk is worth it — and its absence is what the fast path trades.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Detailed Read").await;
    seed_epub_structure(&pool, book_id).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/6!/4/2/1:5)".into()),
            audio_position_seconds: None,
            progress_percent: Some(55),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let progress = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0].resolved.as_ref().expect("resolved");
    // The seeded book has no EPUB on disk, so the walk finds nothing and the
    // read falls back to inverting the stored percent — degrading to a coarser
    // answer, never a wrong one, and saying so.
    assert_eq!(resolved.confidence, PositionConfidence::Low);
    assert_eq!(resolved.chapter_title.as_deref(), Some("Two"));
    assert_eq!(resolved.percent_through_book, Some(55));
}

/// One short chapter body, used for both spine documents of the on-disk
/// fixture below.
const CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C</title></head>
<body>
  <p>First sentence here. Second sentence follows.</p>
</body>
</html>"#;

#[tokio::test]
async fn the_per_book_read_measures_a_real_cfi_against_the_archive_on_disk() {
    // The full path end to end, against an EPUB that really exists: the CFI is
    // walked for its in-document offset, which is what turns "chapter two"
    // into "40% through chapter two".
    let dir = crate::test_support::make_test_dir("resolve_position_full");
    std::fs::write(
        dir.join("book.epub"),
        crate::test_support::build_test_epub_with_nav(
            &[("c1.xhtml", CHAPTER), ("c2.xhtml", CHAPTER)],
            &[("Opening", "c1.xhtml"), ("Closing", "c2.xhtml")],
        ),
    )
    .unwrap();
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library = dir.to_str().unwrap();
    let (book_id, uuid) = seed_named_file(&pool, library, "Real Book", "book.epub").await;
    let user = seed_user(&pool, "alice").await;

    // Extract and store the structure the way the backfill worker does.
    let (file_id, path) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let structure = crate::ebook::toc::extract_structure_from_path(&path)
        .unwrap()
        .expect("the fixture has a spine");
    crate::epub_structure::replace_structure(&pool, file_id, &structure)
        .await
        .unwrap();

    // A position inside the second spine document.
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:20)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let progress = book_progress(&pool, user, &uuid, None)
        .await
        .unwrap()
        .unwrap();
    let resolved = progress.records[0]
        .resolved
        .as_ref()
        .expect("a book with stored structure resolves");
    assert_eq!(resolved.spine_index, Some(1));
    assert_eq!(resolved.chapter_title.as_deref(), Some("Closing"));
    assert_eq!(resolved.chapter_ordinal, Some(2));
    assert_eq!(resolved.chapters_total, Some(2));
    assert_eq!(resolved.confidence, PositionConfidence::High);
    // Exact, deterministic for this fixture, and — the point of the full
    // path — neither figure is answerable without the in-document offset the
    // walk produced. The fast path reports the stored whole-book percent and
    // omits the chapter one entirely.
    assert_eq!(resolved.percent_through_book, Some(72));
    assert_eq!(resolved.percent_through_chapter, Some(45));
}
