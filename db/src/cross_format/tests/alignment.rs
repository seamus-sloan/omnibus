//! `alignment_view`: assembling link state, audio lanes, ebook chapter
//! ticks, the anchor pairs a jump interpolates through, and the batched
//! query count that must not grow with the number of audio files.

use omnibus_shared::cross_format::{CrossFormatLinkMode, DeclareSyncPoint};
use omnibus_shared::ProgressFormat;

use crate::init_db;

use super::super::*;
use super::{
    audio_update, epub_percent_update, seed_audio_chapters, seed_dual_book, seed_epub_chapters,
    seed_user,
};

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
