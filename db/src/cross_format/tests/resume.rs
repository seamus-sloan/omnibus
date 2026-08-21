//! The resume-candidate state machine: not-linked / nothing-newer /
//! candidate / aligned / stale transitions, the fileless-audio-row
//! ambiguity rule, `resume_points`'s per-book collapse, and the
//! short-book equivalence-tolerance helpers it leans on.

use omnibus_shared::cross_format::{CrossFormatLinkMode, CrossFormatResumeState};
use omnibus_shared::ProgressFormat;

use crate::init_db;

use super::super::resume::{audio_equivalence_floor, equivalence_fraction};
use super::super::*;
use super::{audio_update, epub_percent_update, seed_dual_book, seed_user};

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
