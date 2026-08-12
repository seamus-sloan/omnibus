//! Tests for cross-format links and the linear mapping tier: link CRUD and
//! snapshot staleness, sequence/narrations timelines, percent ↔ seconds
//! mapping with inverse consistency, and the resume-candidate state machine.

use omnibus_shared::ProgressUpdate;
use sqlx::SqlitePool;

use crate::init_db;

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

    // The other direction: listening moved past reading → epub candidate.
    progress::upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 50.0, Some(audio[1]), 3_000),
    )
    .await
    .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap();
    assert_eq!(r.state, CrossFormatResumeState::Candidate);
    assert_eq!(r.candidate.unwrap().percent, Some(65));

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
