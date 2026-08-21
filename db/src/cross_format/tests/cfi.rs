//! CFI-precision mapping: a stored `epub_cfi` reading position derives an
//! audio target at the CFI's exact fraction (not a floored integer
//! percent), `derive_candidate_cfi` round-trips through the same ruler the
//! walk uses, and a declared CFI outranks a client-supplied fraction.

use omnibus_shared::cross_format::{CrossFormatLinkMode, CrossFormatResumeState, DeclareSyncPoint};
use omnibus_shared::{ProgressFormat, ProgressUpdate};

use crate::init_db;

use super::super::*;
use super::{audio_update, seed_user};

/// One-paragraph chapter with an odd visible-char count, so mid-chapter
/// fractions never collapse to exact hundredths (which would make the
/// precision assertions vacuous).
const FRACTION_CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C</title></head>
<body>
  <p>First sentence here. Second sentence follows.</p>
</body>
</html>"#;

#[tokio::test]
async fn resume_candidate_audio_target_derives_the_fraction_from_the_stored_cfi() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // A dual-format book whose EPUB really exists on disk, so the spine
    // stats backfill and the CFI walk have a file to read.
    let dir = crate::test_support::make_test_dir("cross_format_cfi_fraction");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(
        dir.join("sub").join("book.epub"),
        crate::test_support::build_test_epub(&[
            ("c1.xhtml", FRACTION_CHAPTER),
            ("c2.xhtml", FRACTION_CHAPTER),
        ]),
    )
    .unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(dir.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let library_id: i64 =
        sqlx::query_scalar("SELECT id FROM scan_roots WHERE display_name = 'lib'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES ('cfi-uuid-1', 'sub/book.epub', ?, 'sub', 'Precise', 'precise') RETURNING id",
    )
    .bind(library_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'EPUB', 'book', 10, 10, 'sub/book.epub')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let audio_id: i64 = sqlx::query_scalar(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
         VALUES (?, 'M4B', 'part0', 100, 1000, 'a0.m4b', 0) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_file_parts
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
         VALUES (?, 0, 'a0.m4b', 100, 1000, 1000.0)",
    )
    .bind(audio_id)
    .execute(&pool)
    .await
    .unwrap();
    crate::indexer::backfill_epub_structure(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();
    let (epub_file_id, _) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let stats = crate::epub_structure::get_spine_stats(&pool, epub_file_id)
        .await
        .unwrap();
    assert!(!stats.is_empty(), "precondition: stats extracted");

    upsert_link(
        &pool,
        user,
        "cfi-uuid-1",
        CrossFormatLinkMode::Sequence,
        None,
    )
    .await
    .unwrap();
    // A CFI-only reading row (no integer percent): 5 visible chars into
    // chapter 2 of two identical chapters — a shade past 50%.
    progress::upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: "cfi-uuid-1".to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:5)".to_string()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(2_000),
        },
    )
    .await
    .unwrap();

    let r = resume_candidate(&pool, user, "cfi-uuid-1", ProgressFormat::Audio)
        .await
        .unwrap();
    // The old integer-percent source would refuse here (the row has no
    // percent); a candidate at all proves the CFI path engaged.
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    let c = r.candidate.unwrap();
    let seconds = c.audio_position_seconds.unwrap();
    // Expected fraction via the same walk the candidate path runs, so the
    // assertion can't disagree with the walk's own offset accounting.
    let (_, epub_path) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let (si, off) = crate::kobo_position::cfi_spine_offset(&epub_path, "epubcfi(/6/4!/4/2/1:5)")
        .unwrap()
        .unwrap();
    let frac = crate::epub_structure::fraction_at(&stats, si as i64, off).unwrap();
    assert!(
        frac > 0.5 && frac < 0.6,
        "mid-chapter-2 offset should sit just past half: {frac}"
    );
    assert!(
        (frac * 100.0).fract() > f64::EPSILON,
        "fixture must produce a non-integer percent or the precision claim is vacuous: {frac}"
    );
    assert!(
        (seconds - frac * 1000.0).abs() < 0.01,
        "mapped seconds {seconds} must carry the CFI fraction {frac} at full precision"
    );
}

/// Seed a dual book whose EPUB really exists on disk (two identical
/// chapters, stats extracted) plus a 1000s single-file audiobook — the
/// scaffolding the CFI-precision tests share.
async fn seed_cfi_book(pool: &sqlx::SqlitePool, tag: &str, uuid: &str) -> i64 {
    let dir = crate::test_support::make_test_dir(tag);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(
        dir.join("sub").join("book.epub"),
        crate::test_support::build_test_epub(&[
            ("c1.xhtml", FRACTION_CHAPTER),
            ("c2.xhtml", FRACTION_CHAPTER),
        ]),
    )
    .unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind(dir.to_str().unwrap())
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
    let library_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE display_name = ?")
        .bind(tag)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES (?, 'sub/book.epub', ?, 'sub', 'Precise', 'precise') RETURNING id",
    )
    .bind(uuid)
    .bind(library_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'EPUB', 'book', 10, 10, 'sub/book.epub')",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    let audio_id: i64 = sqlx::query_scalar(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
         VALUES (?, 'M4B', 'part0', 100, 1000, 'a0.m4b', 0) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_file_parts
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
         VALUES (?, 0, 'a0.m4b', 100, 1000, 1000.0)",
    )
    .bind(audio_id)
    .execute(pool)
    .await
    .unwrap();
    crate::indexer::backfill_epub_structure(pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();
    book_id
}

#[tokio::test]
async fn derive_candidate_cfi_lands_on_the_same_ruler_as_the_walk() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_cfi_book(&pool, "cfi_derive_ruler", "cfi-uuid-2").await;
    let (epub_file_id, epub_path) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let stats = crate::epub_structure::get_spine_stats(&pool, epub_file_id)
        .await
        .unwrap();
    // The fraction of a known mid-chapter-2 CFI, via the same walk.
    let (si, off) = crate::kobo_position::cfi_spine_offset(&epub_path, "epubcfi(/6/4!/4/2/1:5)")
        .unwrap()
        .unwrap();
    let frac = crate::epub_structure::fraction_at(&stats, si as i64, off).unwrap();

    let derived = derive_candidate_cfi(&pool, "cfi-uuid-2", frac)
        .await
        .expect("derivation should succeed with stats and a readable file");
    // Round trip: the derived CFI must resolve back to the same fraction —
    // one ruler, no locations-scale reinterpretation anywhere.
    let (si2, off2) = crate::kobo_position::cfi_spine_offset(&epub_path, &derived)
        .unwrap()
        .unwrap();
    let frac2 = crate::epub_structure::fraction_at(&stats, si2 as i64, off2).unwrap();
    assert!(
        (frac2 - frac).abs() < 1e-9,
        "derived CFI {derived} resolves to {frac2}, expected {frac}"
    );
}

#[tokio::test]
async fn declare_sync_point_prefers_the_declared_cfi_over_the_client_fraction() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let book_id = seed_cfi_book(&pool, "cfi_declare_pref", "cfi-uuid-3").await;
    // The Epub declaration needs a stored audio counterpart to pair with.
    progress::upsert_progress(&pool, user, &audio_update("cfi-uuid-3", 400.0, None, 1_000))
        .await
        .unwrap();

    let link = declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: "cfi-uuid-3".to_string(),
            format: ProgressFormat::Epub,
            // Deliberately wrong locations-scale fraction: the CFI must win.
            ebook_fraction: Some(0.9),
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:5)".to_string()),
            audio_book_file_id: None,
            audio_seconds: None,
        },
    )
    .await
    .unwrap();

    let (_, epub_path) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let (epub_file_id, _) = crate::book_file_with_id(&pool, book_id, "EPUB")
        .await
        .unwrap()
        .unwrap();
    let stats = crate::epub_structure::get_spine_stats(&pool, epub_file_id)
        .await
        .unwrap();
    let (si, off) = crate::kobo_position::cfi_spine_offset(&epub_path, "epubcfi(/6/4!/4/2/1:5)")
        .unwrap()
        .unwrap();
    let expected = crate::epub_structure::fraction_at(&stats, si as i64, off).unwrap();
    let (text_frac, _) = link.user_anchors[0];
    assert!(
        (text_frac - expected).abs() < 1e-9,
        "anchor {text_frac} must come from the CFI ({expected}), not the 0.9 client fraction"
    );
}
