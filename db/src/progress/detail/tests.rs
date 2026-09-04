//! Position-resolution coverage: the pure mapping helpers, then the
//! `book_progress` envelope against a seeded dual-format book.

use omnibus_shared::{ChapterInfo, ProgressUpdate};

use super::*;
use crate::epub_structure::{EbookChapterRow, SpineStatRow};
use crate::init_db;
use crate::progress::upsert_progress;

fn mark(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
    ChapterInfo {
        ordinal,
        title: title.to_string(),
        start_seconds: start,
        duration_seconds: dur,
    }
}

fn spine(spine_index: i64, visible_chars: i64, chars_before: i64) -> SpineStatRow {
    SpineStatRow {
        spine_index,
        href: format!("c{spine_index}.xhtml"),
        visible_chars,
        chars_before,
    }
}

fn chapter(ordinal: i64, title: &str, spine_index: i64, start_chars: i64) -> EbookChapterRow {
    EbookChapterRow {
        ordinal,
        title: title.to_string(),
        href: format!("c{spine_index}.xhtml"),
        spine_index,
        start_chars,
    }
}

// --- pure helpers -----------------------------------------------------

#[test]
fn round2_trims_the_float_noise_a_two_tap_playback_rate_carries() {
    assert_eq!(round2(2.3000000000000003), 2.3);
    assert_eq!(round2(1.0), 1.0);
    assert_eq!(round2(1.755), 1.76);
}

#[test]
fn weakest_reports_the_lower_of_two_confidences() {
    use PositionConfidence::{Approximate, Exact, Unknown};
    assert_eq!(weakest(Exact, Exact), Exact);
    assert_eq!(weakest(Exact, Approximate), Approximate);
    assert_eq!(weakest(Approximate, Exact), Approximate);
    assert_eq!(weakest(Exact, Unknown), Unknown);
    assert_eq!(weakest(Unknown, Approximate), Unknown);
}

#[test]
fn is_synthetic_recognizes_the_sync_layers_one_part_per_chapter_fallback() {
    let synthetic = vec![
        mark(0, "Part 1", 0.0, 600.0),
        mark(1, "Part 2", 600.0, 600.0),
    ];
    assert!(is_synthetic(&synthetic, 2));
}

#[test]
fn is_synthetic_rejects_real_chapter_marks_and_a_mismatched_part_count() {
    let real = vec![
        mark(0, "Chapter One", 0.0, 400.0),
        mark(1, "Chapter Two", 400.0, 400.0),
    ];
    assert!(!is_synthetic(&real, 2));
    // 65 real chapters over a 4-part file: the counts disagree, so the
    // fallback detector must not claim it.
    let named_like_parts = vec![mark(0, "Part 1", 0.0, 10.0), mark(1, "Part 2", 10.0, 10.0)];
    assert!(!is_synthetic(&named_like_parts, 4));
    assert!(!is_synthetic(&[], 0));
}

#[test]
fn mark_number_at_is_one_based_and_independent_of_stored_ordinals() {
    // Container ordinals start at 1 here; the readout must still count from
    // the first mark, not echo a sparse container ordinal.
    let marks = vec![
        mark(1, "a", 0.0, 400.0),
        mark(2, "b", 400.0, 400.0),
        mark(3, "c", 800.0, 400.0),
    ];
    assert_eq!(mark_number_at(&marks, 0.0), Some(1));
    assert_eq!(mark_number_at(&marks, 399.0), Some(1));
    assert_eq!(mark_number_at(&marks, 400.0), Some(2));
    assert_eq!(mark_number_at(&marks, 5_000.0), Some(3));
    assert_eq!(mark_number_at(&[], 10.0), None);
}

#[test]
fn chapter_at_names_the_containing_chapter_exactly_when_it_owns_its_spine_doc() {
    let chapters = vec![
        chapter(0, "One", 0, 0),
        chapter(1, "Two", 1, 100),
        chapter(2, "Three", 2, 300),
    ];
    let (found, confidence) = chapter_at(&chapters, 1, 150);
    assert_eq!(found.map(|c| c.title.as_str()), Some("Two"));
    assert_eq!(confidence, PositionConfidence::Exact);
}

#[test]
fn chapter_at_degrades_to_approximate_when_one_spine_doc_holds_several_chapters() {
    // Three TOC entries in one spine document all record that document's
    // start, so nothing in the data says which of them the reader is in.
    let chapters = vec![
        chapter(0, "One", 0, 0),
        chapter(1, "Two", 0, 0),
        chapter(2, "Three", 0, 0),
    ];
    let (_, confidence) = chapter_at(&chapters, 0, 50);
    assert_eq!(confidence, PositionConfidence::Approximate);
}

#[test]
fn chapter_at_names_nothing_for_a_position_ahead_of_the_first_chapter() {
    let chapters = vec![chapter(0, "One", 2, 500)];
    let (found, _) = chapter_at(&chapters, 0, 10);
    assert!(found.is_none(), "front matter precedes every TOC entry");
}

#[test]
fn percent_through_chapter_measures_against_the_next_chapters_start() {
    let stats = vec![spine(0, 100, 0), spine(1, 200, 100), spine(2, 100, 300)];
    let chapters = vec![
        chapter(0, "One", 0, 0),
        chapter(1, "Two", 1, 100),
        chapter(2, "Three", 2, 300),
    ];
    let pct = percent_through_chapter(&chapters, &stats, &chapters[1], 200).unwrap();
    assert!(
        (pct - 50.0).abs() < 0.001,
        "half of a 100..300 chapter, got {pct}"
    );
}

#[test]
fn percent_through_chapter_is_none_for_a_chapter_with_no_measurable_extent() {
    let stats = vec![spine(0, 100, 0)];
    let chapters = vec![chapter(0, "One", 0, 0), chapter(1, "Two", 0, 0)];
    assert!(percent_through_chapter(&chapters, &stats, &chapters[0], 0).is_none());
}

#[test]
fn resolve_audio_position_names_the_chapter_when_the_marks_are_real() {
    let totals = AudioTotals {
        book_file_id: 1,
        total_duration_seconds: 1200.0,
        audio_part: Some(2),
        audio_part_count: Some(3),
        chapters: vec![
            mark(1, "Chapter One", 0.0, 400.0),
            mark(2, "Chapter Two", 400.0, 400.0),
            mark(3, "Chapter Three", 800.0, 400.0),
        ],
        synthetic_chapters: false,
    };
    let resolved = resolve_audio_position(&totals, 600.0);
    assert_eq!(resolved.chapter_title.as_deref(), Some("Chapter Two"));
    assert_eq!(resolved.chapter_ordinal, Some(2));
    assert_eq!(resolved.chapters_total, Some(3));
    assert_eq!(resolved.confidence, PositionConfidence::Exact);
    assert_eq!(resolved.percent_through_book, Some(50.0));
    assert_eq!(resolved.percent_through_chapter, Some(50.0));
}

#[test]
fn resolve_audio_position_withholds_chapter_vocabulary_for_synthetic_part_marks() {
    // The failure this exists to prevent: a 65-chapter book stored as a
    // 4-part M4B reporting "chapter 4 of 4" — i.e. finished.
    let totals = AudioTotals {
        book_file_id: 1,
        total_duration_seconds: 1000.0,
        audio_part: Some(4),
        audio_part_count: Some(4),
        chapters: vec![
            mark(0, "Part 1", 0.0, 250.0),
            mark(1, "Part 2", 250.0, 250.0),
            mark(2, "Part 3", 500.0, 250.0),
            mark(3, "Part 4", 750.0, 250.0),
        ],
        synthetic_chapters: true,
    };
    let resolved = resolve_audio_position(&totals, 800.0);
    assert!(resolved.chapter_title.is_none());
    assert!(resolved.chapter_ordinal.is_none());
    assert!(resolved.chapters_total.is_none());
    assert_eq!(resolved.confidence, PositionConfidence::Approximate);
    assert_eq!(resolved.percent_through_book, Some(80.0));
}

// --- book_progress ----------------------------------------------------

/// Seed a dual-format book: one audiobook file (1200 s over two parts,
/// three real chapter marks) plus an EPUB row, and give the user a position
/// in each.
async fn seed_dual_format(pool: &sqlx::SqlitePool, user: i64, uuid: &str) {
    super::super::tests::seed_audiobook(pool, uuid).await;
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(47),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(2_000),
        },
    )
    .await
    .expect("seed epub position");
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(1_044.0),
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1_000),
        },
    )
    .await
    .expect("seed audio position");
}

#[tokio::test]
async fn book_progress_returns_every_format_and_names_the_furthest() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;
    seed_dual_format(&pool, user, "dual-1").await;

    let progress = book_progress(&pool, user, "dual-1", None)
        .await
        .expect("book progress");

    assert_eq!(progress.records.len(), 2, "both formats are returned");
    // 87% listened beats 47% read even though the epub row is newer: the
    // question is where the reader is in the book, not what they touched last.
    assert_eq!(progress.furthest, Some(ProgressFormat::Audio));
    let furthest = progress.furthest_record().expect("furthest record");
    assert_eq!(furthest.record.format, ProgressFormat::Audio);
}

#[tokio::test]
async fn book_progress_narrows_to_one_format_when_asked() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;
    seed_dual_format(&pool, user, "dual-1").await;

    let progress = book_progress(&pool, user, "dual-1", Some(ProgressFormat::Epub))
        .await
        .expect("book progress");

    assert_eq!(progress.records.len(), 1);
    assert_eq!(progress.records[0].record.format, ProgressFormat::Epub);
    assert_eq!(progress.furthest, Some(ProgressFormat::Epub));
}

#[tokio::test]
async fn book_progress_gives_every_audio_record_a_runtime_and_a_percent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;
    seed_dual_format(&pool, user, "dual-1").await;

    let progress = book_progress(&pool, user, "dual-1", Some(ProgressFormat::Audio))
        .await
        .expect("book progress");
    let audio = &progress.records[0];

    assert_eq!(audio.total_duration_seconds, Some(1_200.0));
    // Derived on the read path: the write path forbids a client sending one
    // for audio, so without this the field is null and the caller guesses.
    assert_eq!(audio.record.progress_percent, Some(87));
    assert_eq!(audio.resolved.chapter_title.as_deref(), Some("ch"));
    assert_eq!(audio.resolved.chapter_ordinal, Some(3));
    assert_eq!(audio.resolved.chapters_total, Some(3));
    assert_eq!(audio.audio_part, Some(3));
    assert_eq!(audio.audio_part_count, Some(3));
}

#[tokio::test]
async fn book_progress_resolves_an_epub_percent_only_row_as_approximate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;
    seed_dual_format(&pool, user, "dual-1").await;

    let progress = book_progress(&pool, user, "dual-1", Some(ProgressFormat::Epub))
        .await
        .expect("book progress");
    let epub = &progress.records[0];

    // No CFI and no extracted spine stats: the honest answer is the stored
    // percent, flagged as a guess rather than dressed up as a chapter.
    assert_eq!(epub.resolved.percent_through_book, Some(47.0));
    assert_eq!(epub.resolved.confidence, PositionConfidence::Approximate);
    assert!(epub.resolved.chapter_title.is_none());
}

#[tokio::test]
async fn book_progress_is_empty_for_a_book_the_user_has_never_opened() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;
    super::super::tests::seed_audiobook(&pool, "untouched").await;

    let progress = book_progress(&pool, user, "untouched", None)
        .await
        .expect("book progress");

    assert!(progress.records.is_empty());
    assert!(progress.furthest.is_none());
    assert!(!progress.linked);
}

#[tokio::test]
async fn book_progress_is_empty_for_an_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = super::super::tests::seed_user(&pool, "reader").await;

    let progress = book_progress(&pool, user, "no-such-book", None)
        .await
        .expect("book progress");

    assert_eq!(progress.book_uuid, "no-such-book");
    assert!(progress.records.is_empty());
}
