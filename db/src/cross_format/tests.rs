//! Tests for cross-format links and the linear mapping tier: link CRUD and
//! snapshot staleness, sequence/narrations timelines, percent ↔ seconds
//! mapping with inverse consistency, and the resume-candidate state machine.

use omnibus_shared::cross_format::{CrossFormatLinkMode, CrossFormatResumeState, DeclareSyncPoint};
use omnibus_shared::{ProgressFormat, ProgressUpdate};
use sqlx::SqlitePool;

use crate::init_db;

use super::resume::{audio_equivalence_floor, equivalence_fraction};
use super::*;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One dual-format book: an EPUB file plus one M4B `book_files` row per
/// entry of `durations` (ordinals in order, one part each). Returns
/// `(book_id, uuid, audio book_file ids)`.
async fn seed_dual_book(pool: &SqlitePool, durations: &[f64]) -> (i64, String, Vec<i64>) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let library_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES ('book-uuid-1', 'b1.epub', ?, '/lib/b1', 'Dual', 'dual') RETURNING id",
    )
    .bind(library_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'EPUB', 'b1', 10, 10, 'b1.epub')",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    let mut audio_ids = Vec::new();
    for (i, duration) in durations.iter().enumerate() {
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO book_files
                (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
             VALUES (?, 'M4B', ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(book_id)
        .bind(format!("part{i}"))
        .bind(100 + i as i64)
        .bind(1_000 + i as i64)
        .bind(format!("a{i}.m4b"))
        .bind(i as i64)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_file_parts
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
             VALUES (?, 0, ?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(format!("a{i}.m4b"))
        .bind(100 + i as i64)
        .bind(1_000 + i as i64)
        .bind(duration)
        .execute(pool)
        .await
        .unwrap();
        audio_ids.push(file_id);
    }
    (book_id, "book-uuid-1".to_string(), audio_ids)
}

fn epub_percent_update(uuid: &str, percent: i64, clock: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: Some(percent),
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(clock),
    }
}

fn audio_update(uuid: &str, seconds: f64, file_id: Option<i64>, clock: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Audio,
        epub_cfi: None,
        audio_position_seconds: Some(seconds),
        progress_percent: None,
        kobo_location: None,
        book_file_id: file_id,
        client_updated_at: Some(clock),
    }
}

#[tokio::test]
async fn upsert_get_delete_link_round_trips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;

    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert_eq!(link.mode, CrossFormatLinkMode::Sequence);
    assert!(link.audio_snapshot.contains("a0.m4b"));
    assert!(link.confirmed_at > 0);

    // Re-confirming as narrations replaces the row wholesale.
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[0]),
    )
    .await
    .unwrap();
    assert_eq!(link.mode, CrossFormatLinkMode::Narrations);
    assert_eq!(link.primary_book_file_id, Some(audio[0]));

    assert!(delete_link(&pool, user, &uuid).await.unwrap());
    assert!(!delete_link(&pool, user, &uuid).await.unwrap());
    assert!(get_link(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn upsert_link_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let err = upsert_link(&pool, user, "no-such", CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::BookNotFound));
}

#[tokio::test]
async fn get_link_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_link(&pool, 1, "some-uuid").await;
    assert!(matches!(err, Err(CrossFormatError::Sqlx(_))));
}

#[tokio::test]
async fn link_is_stale_flips_when_the_audio_file_set_changes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(!link_is_stale(&pool, book_id, &link).await.unwrap());

    // A replaced file moves its wire etag (mtime/size) → stale.
    sqlx::query("UPDATE book_files SET mtime_epoch = 9999 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(link_is_stale(&pool, book_id, &link).await.unwrap());
}

#[tokio::test]
async fn audio_timeline_concatenates_sequence_files_by_ordinal() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();

    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tl.total_seconds, 1_000.0);
    assert_eq!(
        tl.files
            .iter()
            .map(|f| (f.book_file_id, f.start_seconds))
            .collect::<Vec<_>>(),
        vec![(audio[0], 0.0), (audio[1], 600.0), (audio[2], 900.0)]
    );
}

#[tokio::test]
async fn map_percent_to_audio_lands_in_the_right_file_and_inverts() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(map_percent_to_audio(&tl, 0), Some((audio[0], 0.0)));
    assert_eq!(map_percent_to_audio(&tl, 65), Some((audio[1], 50.0)));
    assert_eq!(map_percent_to_audio(&tl, 95), Some((audio[2], 50.0)));
    // 100% belongs to the last file's end, never past it.
    assert_eq!(map_percent_to_audio(&tl, 100), Some((audio[2], 100.0)));
    assert_eq!(map_percent_to_audio(&tl, 101), None);

    // Inverse consistency within the floor's 1% granularity.
    for pct in [0i64, 13, 42, 65, 95, 100] {
        let (file, secs) = map_percent_to_audio(&tl, pct).unwrap();
        let back = map_audio_to_percent(&tl, file, secs).unwrap();
        assert!(
            (back - pct).abs() <= 1,
            "round trip drifted: {pct} -> {back}"
        );
    }
}

#[tokio::test]
async fn narrations_timeline_aligns_only_the_primary() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 620.0]).await;
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[1]),
    )
    .await
    .unwrap();

    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tl.files.len(), 1);
    assert_eq!(tl.total_seconds, 620.0);
    assert_eq!(tl.files[0].book_file_id, audio[1]);
    // The non-primary narration is deliberately unaligned.
    assert_eq!(map_audio_to_percent(&tl, audio[0], 100.0), None);
}

#[tokio::test]
async fn resume_candidate_walks_the_state_machine() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;

    // Unlinked → sync off.
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::NotLinked);

    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();

    // Linked but no source position → nothing to offer.
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::NothingNewer);

    // Newer reading position → audio candidate at the mapped spot.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 65, 2_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 10.0, Some(audio[0]), 1_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    let c = r.candidate.unwrap();
    assert_eq!(c.book_file_id, Some(audio[1]));
    assert_eq!(c.audio_position_seconds, Some(50.0));
    assert_eq!(c.total_duration_seconds, Some(1_000.0));
    assert_eq!(c.source_client_updated_at, 2_000);

    // The other direction: listening moved well past reading (global 850s
    // = 85% vs the reader's 65%) → epub candidate, marked ahead.
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 250.0, Some(audio[1]), 3_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    let c = r.candidate.unwrap();
    assert_eq!(c.percent, Some(85));
    // The wire also carries the un-floored fraction (global 850s of 1000s).
    assert!((c.fraction.unwrap() - 0.85).abs() < 1e-9);
    assert_eq!(c.source_ahead, Some(true));

    // Target newer than source → nothing newer.
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::NothingNewer);
}

#[tokio::test]
async fn resume_candidate_pauses_on_a_stale_link_and_rejects_unknown_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 2_000))
        .await
        .unwrap();

    sqlx::query("UPDATE book_files SET mtime_epoch = 9999 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::LinkStale);

    let err = resume_candidate(&pool, user, "no-such", ProgressFormat::Audio)
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::BookNotFound));
}

#[test]
fn map_audio_to_percent_refuses_a_zero_length_timeline() {
    let tl = AudioTimeline {
        files: vec![TimelineFile {
            book_file_id: 1,
            start_seconds: 0.0,
            duration_seconds: 0.0,
        }],
        total_seconds: 0.0,
    };
    assert_eq!(map_audio_to_percent(&tl, 1, 0.0), None);
}

#[tokio::test]
async fn resume_candidate_refuses_a_fileless_audio_row_on_a_multi_file_timeline() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    // An audio position with no recorded file: ambiguous across two files.
    progress::upsert_progress(&pool, user, &audio_update(&uuid, 50.0, None, 3_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(
        r.state,
        CrossFormatResumeState::NothingNewer,
        "an ambiguous source must refuse, not guess the first file"
    );
}

#[tokio::test]
async fn resume_candidate_accepts_a_fileless_audio_row_on_a_single_file_timeline() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[600.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &audio_update(&uuid, 300.0, None, 3_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    assert_eq!(r.candidate.unwrap().percent, Some(50));
}

#[tokio::test]
async fn alignment_view_assembles_link_positions_and_audio_lanes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 65, 2_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 42.0, Some(audio[0]), 1_000),
    )
    .await
    .unwrap();

    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    let link = view.link.unwrap();
    assert!(!link.stale);
    assert_eq!(link.mode, CrossFormatLinkMode::Sequence);
    assert_eq!(view.audio_files.len(), 2);
    assert_eq!(view.audio_files[0].duration_seconds, 600.0);
    // No spine stats seeded: the text lane is honestly absent, and the
    // synthetic-chapter-free audio files carry no tick offsets.
    assert!(view.ebook.is_none());
    assert!(view.audio_files[0].chapter_starts.is_empty());
    assert_eq!(view.reading.unwrap().percent, Some(65));
    assert_eq!(view.listening.unwrap().seconds, 42.0);
}

#[tokio::test]
async fn alignment_view_reports_ebook_ticks_when_structure_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[600.0]).await;
    let epub_file: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE format = 'EPUB' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    crate::epub_structure::replace_structure(
        &pool,
        epub_file,
        &crate::ebook::toc::EpubStructure {
            spine: vec![
                crate::ebook::toc::SpineStat {
                    spine_index: 0,
                    href: "c1.xhtml".into(),
                    visible_chars: 40,
                },
                crate::ebook::toc::SpineStat {
                    spine_index: 1,
                    href: "c2.xhtml".into(),
                    visible_chars: 60,
                },
            ],
            chapters: vec![crate::ebook::toc::TocChapter {
                ordinal: 0,
                title: "Chapter Two".into(),
                href: "c2.xhtml".into(),
                spine_index: 1,
                start_chars: 40,
            }],
        },
    )
    .await
    .unwrap();

    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    let ebook = view.ebook.unwrap();
    assert_eq!(ebook.total_chars, 100);
    assert_eq!(ebook.chapters.len(), 1);
    assert_eq!(ebook.chapters[0].title, "Chapter Two");
    assert!((ebook.chapters[0].percent - 40.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn set_audio_order_persists_and_refuses_a_mismatched_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;

    set_audio_order(&pool, &uuid, &[audio[1], audio[0]])
        .await
        .unwrap();
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        view.audio_files
            .iter()
            .map(|f| f.book_file_id)
            .collect::<Vec<_>>(),
        vec![audio[1], audio[0]]
    );

    // Anything that isn't exactly the current set refuses with the
    // dedicated mismatch variant: missing file, foreign id, duplicate.
    for bad in [
        vec![audio[0]],
        vec![audio[0], 999_999],
        vec![audio[0], audio[0]],
    ] {
        assert!(matches!(
            set_audio_order(&pool, &uuid, &bad).await.unwrap_err(),
            CrossFormatError::AudioSetMismatch
        ));
    }
}

// ── chapter-anchored tier ───────────────────────────────────────────

async fn seed_epub_chapters(pool: &SqlitePool, titles: &[(&str, i64)], per_chapter: i64) {
    let epub_file: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE format = 'EPUB' LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    let spine = titles
        .iter()
        .enumerate()
        .map(|(i, _)| crate::ebook::toc::SpineStat {
            spine_index: i as i64,
            href: format!("c{i}.xhtml"),
            visible_chars: per_chapter,
        })
        .collect();
    let chapters = titles
        .iter()
        .enumerate()
        .map(|(i, (title, start))| crate::ebook::toc::TocChapter {
            ordinal: i as i64,
            title: (*title).to_string(),
            href: format!("c{i}.xhtml"),
            spine_index: i as i64,
            start_chars: *start,
        })
        .collect();
    crate::epub_structure::replace_structure(
        pool,
        epub_file,
        &crate::ebook::toc::EpubStructure { spine, chapters },
    )
    .await
    .unwrap();
}

async fn seed_audio_chapters(pool: &SqlitePool, file_id: i64, chapters: &[(&str, f64)]) {
    for (i, (title, start)) in chapters.iter().enumerate() {
        sqlx::query(
            "INSERT INTO file_chapters (book_file_id, ordinal, title, start_seconds, duration_seconds)
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(file_id)
        .bind(i as i64)
        .bind(title)
        .bind(start)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[test]
fn is_synthetic_title_matches_only_the_indexer_fallback() {
    use super::anchors::is_synthetic_title;
    assert!(is_synthetic_title("Part 1"));
    assert!(is_synthetic_title("Part 42"));
    assert!(!is_synthetic_title("Part"));
    assert!(!is_synthetic_title("Party 3"));
    assert!(!is_synthetic_title("Chapter 1"));
}

#[test]
fn interpolate_maps_piecewise_through_anchors_in_both_directions() {
    use super::anchors::{interpolate, Anchor};
    let anchors = vec![Anchor {
        text_frac: 0.2,
        audio_frac: 0.4,
    }];
    assert!((interpolate(&anchors, 0.1, true) - 0.2).abs() < 1e-9);
    assert!((interpolate(&anchors, 0.2, true) - 0.4).abs() < 1e-9);
    assert!((interpolate(&anchors, 0.6, true) - 0.7).abs() < 1e-9);
    // Inverse direction mirrors.
    assert!((interpolate(&anchors, 0.4, false) - 0.2).abs() < 1e-9);
    assert!((interpolate(&anchors, 0.7, false) - 0.6).abs() < 1e-9);
}

#[tokio::test]
async fn resume_candidate_uses_chapter_anchors_when_titles_match() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    // Text chapters at 0 / one-third / two-thirds; the narration spends
    // disproportionate time early (Bravo at 300s, Charlie at 480s of 600).
    seed_epub_chapters(&pool, &[("Alpha", 0), ("Bravo", 40), ("Charlie", 80)], 40).await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[("Alpha", 0.0), ("Bravo", 300.0), ("Charlie", 480.0)],
    )
    .await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 33, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored
    );
    // Anchored: 33% of text ≈ the Bravo anchor → ≈ 300s (linear would say
    // 198s). Allow a little slack for the 33-vs-1/3 floor.
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (295.0..=305.0).contains(&secs),
        "expected the anchored map near 300s, got {secs}"
    );

    // Inverse: listening at Charlie's start maps back to two-thirds text.
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 480.0, Some(audio[0]), 3_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored
    );
    let pct = c.percent.unwrap();
    assert!((65..=67).contains(&pct), "expected ≈ 66%, got {pct}");
}

#[tokio::test]
async fn anchoring_degrades_to_linear_when_nothing_trustworthy_matches() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    // Three text chapters vs two synthetic audio marks: no titles, no
    // count alignment — the linear tier must answer.
    seed_epub_chapters(&pool, &[("Alpha", 0), ("Bravo", 40), ("Charlie", 80)], 40).await;
    seed_audio_chapters(&pool, audio[0], &[("Part 1", 0.0), ("Part 2", 300.0)]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::Linear
    );
    assert_eq!(c.audio_position_seconds, Some(300.0));
}

#[tokio::test]
async fn per_chapter_mp3_stems_anchor_when_chapters_are_synthetic() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // One audio file with three per-chapter parts (the MP3-folder shape):
    // synthetic chapter titles, real names in the filename stems.
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    sqlx::query("DELETE FROM book_file_parts WHERE book_file_id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    for (i, (name, dur)) in [
        ("01 - Alpha.mp3", 100.0),
        ("02 - Bravo.mp3", 350.0),
        ("03 - Charlie.mp3", 150.0),
    ]
    .iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO book_file_parts
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
             VALUES (?, ?, ?, 1, 1, ?)",
        )
        .bind(audio[0])
        .bind(i as i64)
        .bind(name)
        .bind(dur)
        .execute(&pool)
        .await
        .unwrap();
    }
    seed_audio_chapters(
        &pool,
        audio[0],
        &[("Part 1", 0.0), ("Part 2", 100.0), ("Part 3", 450.0)],
    )
    .await;
    seed_epub_chapters(&pool, &[("Alpha", 0), ("Bravo", 40), ("Charlie", 80)], 40).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 33, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored
    );
    // 33% of text ≈ the "Bravo" stem anchor at 100s (linear: 198s).
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (95.0..=105.0).contains(&secs),
        "expected the stem-anchored map near 100s, got {secs}"
    );
}

#[tokio::test]
async fn alignment_view_reports_the_anchor_match_even_before_linking() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    seed_epub_chapters(&pool, &[("Alpha", 0), ("Bravo", 40), ("Charlie", 80)], 40).await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[("Alpha", 0.0), ("Bravo", 300.0), ("Charlie", 480.0)],
    )
    .await;

    // Unlinked: the preview still evaluates against the default sequence
    // declaration so the modal can promise chapter accuracy honestly.
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    let m = view.anchor_match.unwrap();
    assert_eq!((m.matched, m.ebook_chapters), (3, 3));
    assert_eq!(
        m.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored
    );
}

#[tokio::test]
async fn resume_points_collapse_a_linked_book_to_one_card_with_the_counterpart() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 2_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 60.0, Some(audio[0]), 1_000),
    )
    .await
    .unwrap();

    // Unlinked: the book competes with itself — two cards, neither linked.
    let before = progress::resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|p| !p.linked && p.cross_format.is_none()));

    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let after = progress::resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(after.len(), 1, "linked books collapse to the newest card");
    let card = &after[0];
    assert_eq!(card.record.format, ProgressFormat::Epub);
    assert!(card.linked);
    let counterpart = card.cross_format.as_ref().unwrap();
    assert_eq!(counterpart.target, ProgressFormat::Audio);
    assert_eq!(counterpart.audio_position_seconds, Some(300.0));
    assert_eq!(counterpart.book_file_id, Some(audio[0]));
}

#[tokio::test]
async fn resume_points_collapse_marks_a_stale_link_without_a_counterpart() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 2_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 60.0, Some(audio[0]), 1_000),
    )
    .await
    .unwrap();
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    sqlx::query("UPDATE book_files SET mtime_epoch = 9999 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();

    let points = progress::resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points.len(), 1, "a stale link still collapses");
    assert!(points[0].linked);
    assert!(
        points[0].cross_format.is_none(),
        "mapping is paused while stale — no counterpart affordance"
    );
}

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
async fn seed_cfi_book(pool: &SqlitePool, tag: &str, uuid: &str) -> i64 {
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

#[tokio::test]
async fn resume_candidate_aligns_when_the_target_already_sits_at_the_mapped_spot() {
    // The live We Are Legion case: reader at 19%, listener at the mapped
    // equivalent, audio clock newer — the old clock-only gate offered
    // "jump to ≈19%" while at 19%.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 19, 1_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 190.0, Some(audio[0]), 2_000),
    )
    .await
    .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Aligned);
    // The mapped position still rides along for navigation affordances.
    assert_eq!(r.candidate.as_ref().unwrap().percent, Some(19));

    // Well outside the tolerance: a genuine advance still offers, ahead.
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 400.0, Some(audio[0]), 3_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    assert_eq!(r.candidate.unwrap().source_ahead, Some(true));
}

#[tokio::test]
async fn resume_candidate_marks_backward_offers_instead_of_claiming_further() {
    // A newer-clocked source that sits *behind* the target (deliberate
    // re-listening) survives the gate but is flagged, so prompt copy can
    // say "back" instead of the false "past this page".
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 60, 1_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 100.0, Some(audio[0]), 2_000),
    )
    .await
    .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    assert_eq!(r.candidate.unwrap().source_ahead, Some(false));
}

#[tokio::test]
async fn resume_candidate_audio_target_aligns_and_stands_aside_without_a_placeable_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    // Reader at 65% (newer); listener already at the mapped global 650s.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 65, 2_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 50.0, Some(audio[1]), 1_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Aligned);
    assert_eq!(r.candidate.as_ref().unwrap().book_file_id, Some(audio[1]));

    // A file-less audio row on a multi-file timeline can't be placed, so
    // the gate stands aside rather than guessing: the offer survives.
    // Since #1889's write-path backstop preserves a stored file id against
    // NULL overwrites, the shape is only reachable as a *first* write —
    // use a fresh user.
    let bob = seed_user(&pool, "bob").await;
    upsert_link(&pool, bob, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, bob, &audio_update(&uuid, 50.0, None, 1_500))
        .await
        .unwrap();
    progress::upsert_progress(&pool, bob, &epub_percent_update(&uuid, 65, 2_500))
        .await
        .unwrap();
    let r = resume_candidate(&pool, bob, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    assert_eq!(r.candidate.unwrap().source_ahead, None);
}

#[test]
fn equivalence_fraction_widens_for_short_books_and_caps() {
    // No stats → the 0.5% base.
    assert!((equivalence_fraction(None) - 0.005).abs() < 1e-12);
    // A long book keeps the base (two locations are a sliver).
    assert!((equivalence_fraction(Some(1_000_000)) - 0.005).abs() < 1e-12);
    // A short book widens to two reader locations…
    let short = equivalence_fraction(Some(200_000));
    assert!((short - 2.0 * 1024.0 / 200_000.0).abs() < 1e-12);
    // …and a tiny one caps at 5% so offers can still exist at all.
    assert!((equivalence_fraction(Some(10_000)) - 0.05).abs() < 1e-12);
}

#[test]
fn audio_equivalence_floor_scales_down_for_short_books() {
    // Real audiobook: the full minute of seek noise.
    assert!((audio_equivalence_floor(36_000.0) - 60.0).abs() < 1e-9);
    // Short book: 2% of the timeline, so a fixture-length file still
    // distinguishes "same spot" from a genuine offer.
    assert!((audio_equivalence_floor(60.0) - 1.2).abs() < 1e-9);
    assert_eq!(audio_equivalence_floor(0.0), 0.0);
}

#[test]
fn chapter_number_parses_real_title_shapes_and_refuses_junk() {
    use super::anchors::chapter_number;
    assert_eq!(chapter_number("Chapter One: The Pigeon Drop"), Some(1));
    assert_eq!(
        chapter_number("Chapter One: The Pigeon Drop: In Which Saeldian Is Given an Offer"),
        Some(1)
    );
    assert_eq!(chapter_number("Chapter 21 - The Long Road"), Some(21));
    assert_eq!(chapter_number("chapter twenty-one"), Some(21));
    // Spaced and Unicode-hyphen composites must not truncate to the tens.
    assert_eq!(chapter_number("Chapter Twenty One"), Some(21));
    assert_eq!(chapter_number("chapter twenty\u{2011}one"), Some(21));
    assert_eq!(chapter_number("Chapter Twenty: The Storm"), Some(20));
    assert_eq!(chapter_number("Chapter Ninety"), Some(90));
    assert_eq!(chapter_number("03 - Chapter Three"), Some(3));
    assert_eq!(chapter_number("Chapter1"), Some(1));
    assert_eq!(chapter_number("Chapterhouse: Dune"), None);
    assert_eq!(chapter_number("Part 1"), None);
    assert_eq!(chapter_number("STORMLIGHT0501P01"), None);
    assert_eq!(chapter_number("Prologue: To Live"), None);
}

#[tokio::test]
async fn anchoring_pairs_subtitle_decorated_chapters_by_number_despite_front_matter() {
    // The Feywild Job shape (#1894): audio marks carry full decorated
    // titles, the nav is terser and front-matter-heavy — exact equality
    // finds nothing, but the chapter-number rung pairs them.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    seed_epub_chapters(
        &pool,
        &[
            ("Cover", 0),
            ("Title Page", 0),
            ("Copyright", 0),
            ("Contents", 0),
            ("Chapter One: The Pigeon Drop", 0),
            ("Chapter Two: A Better Liar", 40),
            ("Chapter Three: Plan Faster", 80),
            ("Chapter Four: The Feywild", 120),
        ],
        40,
    )
    .await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[
            (
                "Chapter One: The Pigeon Drop: In Which Saeldian Is Given an Offer",
                0.0,
            ),
            (
                "Chapter Two: A Better Liar than Anyone: In Which Kell Is Offered",
                250.0,
            ),
            ("Chapter Three: Plan Faster: Wherein We Learn", 400.0),
            ("Chapter Four: The Feywild: At Last", 520.0),
        ],
    )
    .await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 25, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored,
        "the chapter-number rung must anchor the decorated-title shape"
    );
    // 25% of text = Chapter Three's start (80 of the 320 total chars —
    // front-matter spine entries count) → ≈ 400s; linear would say 150s.
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (395.0..=405.0).contains(&secs),
        "expected the anchored map near 400s, got {secs}"
    );
}

#[tokio::test]
async fn anchoring_pairs_terse_nav_against_decorated_marks_by_prefix() {
    // No chapter numbering at all — the prefix rung carries books whose
    // marks decorate the nav's title with a subtitle.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    seed_epub_chapters(
        &pool,
        &[
            ("Once Long Ago", 0),
            ("The Wishing Rock", 40),
            ("Milkweed and Amulets", 80),
            ("The Long Way Home", 120),
        ],
        40,
    )
    .await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[
            ("Once Long Ago: A Prologue", 0.0),
            ("The Wishing Rock: Where It Started", 250.0),
            ("Milkweed and Amulets: A Bargain", 400.0),
            ("The Long Way Home: An Ending", 520.0),
        ],
    )
    .await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 25, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::ChapterAnchored,
        "the prefix rung must anchor subtitle-decorated titles"
    );
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (245.0..=255.0).contains(&secs),
        "expected ≈250s, got {secs}"
    );
}

#[tokio::test]
async fn anchoring_still_refuses_junk_encoder_labels() {
    // The Wind and Truth shape: rip-tool part labels are not titles; no
    // rung may invent a match, and the modal learns marks exist anyway.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    seed_epub_chapters(
        &pool,
        &[
            ("Prologue: To Live", 0),
            ("Day One", 30),
            ("The Vow", 60),
            ("Interlude", 90),
            ("Day Two", 120),
        ],
        30,
    )
    .await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[
            ("STORMLIGHT0501P01", 0.0),
            ("STORMLIGHT0501P02", 200.0),
            ("STORMLIGHT0501P03", 400.0),
        ],
    )
    .await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 2_000))
        .await
        .unwrap();

    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::Linear,
        "junk labels must not anchor"
    );

    // The alignment view distinguishes "marks exist, couldn't align"
    // (nonzero count, no match) from "no marks at all".
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    assert!(view.anchor_match.is_none());
    assert_eq!(view.audio_chapter_marks, 3);
}

fn declare_audio(uuid: &str, file_id: Option<i64>, seconds: f64) -> DeclareSyncPoint {
    DeclareSyncPoint {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Audio,
        ebook_fraction: None,
        epub_cfi: None,
        audio_book_file_id: file_id,
        audio_seconds: Some(seconds),
    }
}

#[tokio::test]
async fn declare_sync_point_records_the_anchor_and_turns_follow_on() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Single audio file: the declaration may auto-confirm a sequence link.
    let (_, uuid, _) = seed_dual_book(&pool, &[1_000.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();

    let link = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap();
    assert!(link.follow);
    assert_eq!(link.user_anchors.len(), 1);
    let (t, s) = link.user_anchors[0];
    assert!((t - 0.40).abs() < 1e-9);
    assert!((s - 500.0).abs() < 1e-9);

    // The anchor bends the mapping: reading at 40% now maps to 500s
    // (linear would say 400s), and the resume carries follow + the
    // user-anchored confidence.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 2_000))
        .await
        .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert!(r.follow);
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::UserAnchored
    );
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (495.0..=505.0).contains(&secs),
        "user anchor must pin 40% to ≈500s, got {secs}"
    );

    // Re-declaring nearby replaces the pair instead of stacking.
    let link = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 520.0))
        .await
        .unwrap();
    assert_eq!(link.user_anchors.len(), 1);
    assert!((link.user_anchors[0].1 - 520.0).abs() < 1e-9);
}

#[tokio::test]
async fn declare_sync_point_refuses_what_it_cannot_pair() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 400.0]).await;

    // Multi-file with no link: never guess an order.
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::LinkRequired));

    // Linked, but the counterpart has no reading position to pair with.
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::CounterpartMissing));

    // A stale audio set pauses declarations exactly like offers.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 10, 1_000))
        .await
        .unwrap();
    sqlx::query("UPDATE book_files SET mtime_epoch = 4242 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::AudioSetMismatch));
}

#[tokio::test]
async fn reconfirm_keeps_follow_and_clears_anchors_only_when_the_audio_set_changed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap();

    // Same audio set: re-confirming keeps follow and the anchors.
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.follow);
    assert_eq!(link.user_anchors.len(), 1);

    // Changed audio set: anchors' global seconds mean nothing on the new
    // timeline — cleared; the follow opt-in survives.
    sqlx::query("UPDATE book_files SET mtime_epoch = 777 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.follow);
    assert!(link.user_anchors.is_empty());
}

#[tokio::test]
async fn reordering_audio_files_reads_as_a_different_set_and_clears_anchors() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 500.0))
        .await
        .unwrap();

    // Swap the two files' ordinals — the concatenated timeline changes,
    // so every stored global second now means a different spot. The
    // snapshot must read as a different set: the link goes stale for
    // everyone, and a re-confirm clears the old-ruler anchors.
    // Three steps through a parking ordinal: the UNIQUE(book_id, format,
    // ordinal) constraint is enforced per-row, so a single-statement swap
    // trips it mid-UPDATE.
    for (id, ordinal) in [(audio[0], 99), (audio[1], 0), (audio[0], 1)] {
        sqlx::query("UPDATE book_files SET ordinal = ? WHERE id = ?")
            .bind(ordinal)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let book_id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();
    let link = get_link(&pool, user, &uuid).await.unwrap().unwrap();
    assert!(link_is_stale(&pool, book_id, &link).await.unwrap());

    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.user_anchors.is_empty());
}

#[tokio::test]
async fn reconfirm_with_a_different_mode_or_primary_clears_anchors() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 500.0))
        .await
        .unwrap();

    // Same files, different mode: the anchors' global seconds were
    // measured on the sequence concatenation — meaningless against a
    // narrations primary. Cleared.
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[0]),
    )
    .await
    .unwrap();
    assert!(link.user_anchors.is_empty());

    // Re-anchor on the narrations primary, then switch primaries: a
    // different recording is a different ruler too. Cleared again.
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap();
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[1]),
    )
    .await
    .unwrap();
    assert!(link.user_anchors.is_empty());
}

#[tokio::test]
async fn set_follow_flips_only_existing_links() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[600.0]).await;
    assert!(!set_follow(&pool, user, &uuid, true).await.unwrap());
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(set_follow(&pool, user, &uuid, true).await.unwrap());
    assert!(get_link(&pool, user, &uuid).await.unwrap().unwrap().follow);
    assert!(set_follow(&pool, user, &uuid, false).await.unwrap());
    assert!(!get_link(&pool, user, &uuid).await.unwrap().unwrap().follow);
}

#[test]
fn merge_user_anchors_outranks_conflicting_chapter_anchors() {
    use super::anchors::{merge_user_anchors, Anchor, AnchorMap};
    let user = [Anchor {
        text_frac: 0.5,
        audio_frac: 0.7,
    }];
    let chapter = AnchorMap {
        anchors: vec![
            // Fits strictly before the user pair on both axes: kept.
            Anchor {
                text_frac: 0.2,
                audio_frac: 0.3,
            },
            // Violates monotonicity against the user pair (audio side
            // sits past it while text sits before): dropped.
            Anchor {
                text_frac: 0.4,
                audio_frac: 0.8,
            },
            // Fits strictly after: kept.
            Anchor {
                text_frac: 0.9,
                audio_frac: 0.95,
            },
        ],
        matched: 3,
        ebook_chapters: 10,
    };
    let merged = merge_user_anchors(&user, Some(chapter)).unwrap();
    assert_eq!(
        merged
            .anchors
            .iter()
            .map(|a| (a.text_frac, a.audio_frac))
            .collect::<Vec<_>>(),
        vec![(0.2, 0.3), (0.5, 0.7), (0.9, 0.95)]
    );
    // No user anchors → chapter map passes through untouched.
    assert!(merge_user_anchors(&[], None).is_none());
}

#[tokio::test]
async fn alignment_view_serves_the_anchor_pairs_the_jump_uses() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;
    seed_epub_chapters(&pool, &[("Alpha", 0), ("Bravo", 40), ("Charlie", 80)], 40).await;
    seed_audio_chapters(
        &pool,
        audio[0],
        &[("Alpha", 0.0), ("Bravo", 300.0), ("Charlie", 480.0)],
    )
    .await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();

    // Chapter-anchored: the modal preview interpolates through exactly
    // these pairs — the mapping the jump runs.
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        view.anchor_pairs
            .iter()
            .map(|(t, a)| ((t * 120.0).round(), (a * 600.0).round()))
            .collect::<Vec<_>>(),
        // Ticks at chapter starts: Alpha (0,0), Bravo (40/120 chars,
        // 300s), Charlie (80/120, 480s).
        vec![(0.0, 0.0), (40.0, 300.0), (80.0, 480.0)]
    );

    // A user anchor joins (and outranks) the served pairs.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 50, 1_000))
        .await
        .unwrap();
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 390.0, Some(audio[0]), 2_000),
    )
    .await
    .unwrap();
    declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            ebook_fraction: Some(0.5),
            epub_cfi: None,
            audio_book_file_id: None,
            audio_seconds: None,
        },
    )
    .await
    .unwrap();
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    assert!(
        view.anchor_pairs
            .iter()
            .any(|(t, a)| (t - 0.5).abs() < 1e-9 && (a - 0.65).abs() < 1e-9),
        "the declared (0.5, 390s/600s) pair must be served: {:?}",
        view.anchor_pairs
    );
}

// --- #2019: audio_marks batching + alignment_view's single audio_marks fetch ---

/// Counts `tracing` events sqlx emits (target `"sqlx::query"`, one per
/// executed statement) while installed as the default subscriber. Mirrors
/// the `QueryCounter` pattern in `db/src/epub_rewrite/tests.rs`; every
/// `Subscriber` method besides `event` is a no-op.
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

/// The query count `alignment_view` issues against a fresh in-memory pool
/// seeded with `file_count` sequence-mode audio files (no per-file
/// chapters — the old per-file `hls::get_chapters`/`get_parts` loop still
/// issued one round trip per file even when every result came back empty).
async fn alignment_view_query_count(file_count: usize) -> usize {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let durations: Vec<f64> = vec![300.0; file_count];
    let (_, uuid, _) = seed_dual_book(&pool, &durations).await;

    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guard = tracing::subscriber::set_default(QueryCounter(count.clone()));
    alignment_view(&pool, user, &uuid).await.unwrap();
    drop(guard);
    count.load(std::sync::atomic::Ordering::SeqCst)
}

#[tokio::test]
async fn alignment_view_issues_a_query_count_independent_of_audio_file_count() {
    // `audio_marks` used to issue one (or two) `hls` queries per timeline
    // file, and `alignment_view` computed it twice over (once via
    // `usable_audio_marks`, once inside `anchor_map`) — a sequence-mode
    // book laid out as one file per chapter meant dozens of round trips
    // per modal open. Both are batched and deduped now, so the query count
    // must not grow with the file count.
    let few = alignment_view_query_count(2).await;
    let many = alignment_view_query_count(40).await;
    assert_eq!(
        few, many,
        "alignment_view's query count must not grow with the number of \
         audio files in the timeline: {few} queries for 2 files vs {many} \
         for 40"
    );
}

#[test]
fn interpolate_extends_the_measured_slope_past_two_or_more_anchors() {
    use super::anchors::{interpolate, Anchor};
    // Two anchors measure a local rate of 1.0 audio per text; past the
    // last one the map extends that rate — text 0.75 lands at 0.65, not
    // at the endpoint-linear 0.7.
    let anchors = vec![
        Anchor {
            text_frac: 0.4,
            audio_frac: 0.3,
        },
        Anchor {
            text_frac: 0.5,
            audio_frac: 0.4,
        },
    ];
    let mapped = interpolate(&anchors, 0.75, true);
    assert!(
        (mapped - 0.65).abs() < 1e-9,
        "expected measured-slope extension 0.65, got {mapped}"
    );
    // Clamped at the end of the book, never past it.
    assert!(interpolate(&anchors, 1.0, true) <= 1.0);
    // A single anchor keeps the endpoint-linear behavior — one point has
    // no measured slope to extend.
    let single = vec![Anchor {
        text_frac: 0.2,
        audio_frac: 0.4,
    }];
    assert!((interpolate(&single, 0.6, true) - 0.7).abs() < 1e-9);
}

#[tokio::test]
async fn declare_sync_point_accumulates_nearby_anchors() {
    // Two declarations 1% of the book apart must BOTH survive — on a
    // 50-hour timeline the old 2% replace slack silently discarded
    // re-syncs an hour apart (#1954).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            ebook_fraction: None,
            epub_cfi: None,
            audio_book_file_id: Some(audio[0]),
            audio_seconds: Some(400.0),
        },
    )
    .await
    .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 41, 2_000))
        .await
        .unwrap();
    let link = declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            ebook_fraction: None,
            epub_cfi: None,
            audio_book_file_id: Some(audio[0]),
            audio_seconds: Some(410.0),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        link.user_anchors.len(),
        2,
        "a re-sync 1% away must accumulate, not replace: {:?}",
        link.user_anchors
    );
}

#[test]
fn chapter_number_parses_bare_ordinals_and_noisy_audio_marks() {
    use super::anchors::chapter_number;
    // The Bobiverse shape (live #1954 evidence): the ebook nav numbers
    // chapters without the word, the audio marks pad and append a runtime.
    assert_eq!(chapter_number("1.\u{9}Sky God"), Some(1));
    assert_eq!(chapter_number("12. Colony Site"), Some(12));
    assert_eq!(chapter_number("3) Life in Camelot"), Some(3));
    assert_eq!(chapter_number(" Chapter 001  - 00:17:05"), Some(1));
    // A title that merely IS a number never reads as its chapter.
    assert_eq!(chapter_number("1984"), None);
    // Front matter stays unnumbered.
    assert_eq!(chapter_number("Acknowledgements"), None);
    assert_eq!(chapter_number("Table of Contents"), None);
}
