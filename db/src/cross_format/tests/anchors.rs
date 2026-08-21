//! The chapter-anchored mapping tier: title/prefix/number pairing rungs,
//! synthetic-chapter and encoder-junk detection, piecewise interpolation
//! (including past the last anchor), and user-anchor precedence over the
//! chapter map.

use omnibus_shared::cross_format::CrossFormatLinkMode;
use omnibus_shared::ProgressFormat;

use crate::init_db;

use super::super::*;
use super::{
    audio_update, epub_percent_update, seed_audio_chapters, seed_dual_book, seed_epub_chapters,
    seed_user,
};

#[test]
fn is_synthetic_title_matches_only_the_indexer_fallback() {
    use super::super::anchors::is_synthetic_title;
    assert!(is_synthetic_title("Part 1"));
    assert!(is_synthetic_title("Part 42"));
    assert!(!is_synthetic_title("Part"));
    assert!(!is_synthetic_title("Party 3"));
    assert!(!is_synthetic_title("Chapter 1"));
}

#[test]
fn interpolate_maps_piecewise_through_anchors_in_both_directions() {
    use super::super::anchors::{interpolate, Anchor};
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

#[test]
fn chapter_number_parses_real_title_shapes_and_refuses_junk() {
    use super::super::anchors::chapter_number;
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

#[test]
fn merge_user_anchors_outranks_conflicting_chapter_anchors() {
    use super::super::anchors::{merge_user_anchors, Anchor, AnchorMap};
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

#[test]
fn interpolate_extends_the_measured_slope_past_two_or_more_anchors() {
    use super::super::anchors::{interpolate, Anchor};
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

#[test]
fn chapter_number_parses_bare_ordinals_and_noisy_audio_marks() {
    use super::super::anchors::chapter_number;
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
