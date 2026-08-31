//! Tests for the mobile player's per-render display derivation — pins the
//! scrubber row's book-time position readouts and rate-adjusted "left"
//! estimates at the `derive_player_state` boundary.

use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::derive_player_state;
use super::state::SleepState;
use super::view::PlayerView;

/// Two 30-minute chapters; the values every test below derives from.
fn two_chapter_view() -> PlayerView {
    let chapters = vec![
        ChapterInfo {
            ordinal: 1,
            title: "One".into(),
            start_seconds: 0.0,
            duration_seconds: 1800.0,
        },
        ChapterInfo {
            ordinal: 2,
            title: "Two".into(),
            start_seconds: 1800.0,
            duration_seconds: 1800.0,
        },
    ];
    PlayerView::from_direct(&EbookMetadata::default(), chapters, 3600.0, vec![])
}

#[test]
fn derive_player_state_reads_book_time_and_scales_only_the_time_left() {
    // Issue #2344: 20 book-minutes into the first of two 30-minute chapters at
    // 2x. The position readouts (elapsed, chapter-elapsed, chapter-duration)
    // show real book-time and do NOT change with the rate — they match the
    // bookmark stamps and the book detail page. Only the "left" values scale.
    let d = derive_player_state(
        &two_chapter_view(),
        1200.0,
        3600.0,
        0,
        2.0,
        SleepState::Off,
        None,
    );
    // Book-time position — identical to the 1x derivation below.
    assert!((d.elapsed_book - 1200.0).abs() < f64::EPSILON);
    assert!((d.within - 1200.0).abs() < f64::EPSILON);
    assert!((d.chapter_dur - 1800.0).abs() < f64::EPSILON);
    // "Left" estimates are rate-adjusted — halved at 2x.
    assert!((d.chapter_left - 300.0).abs() < f64::EPSILON);
    assert!((d.remaining_left - 1200.0).abs() < f64::EPSILON);
    // The seek coordinates stay book-time — the range input's value/max.
    assert!((d.effective - 1200.0).abs() < f64::EPSILON);
    assert!((d.scrub_max - 3600.0).abs() < f64::EPSILON);
}

#[test]
fn derive_player_state_keeps_book_time_labels_at_1x() {
    let d = derive_player_state(
        &two_chapter_view(),
        1200.0,
        3600.0,
        0,
        1.0,
        SleepState::Off,
        None,
    );
    assert!((d.elapsed_book - 1200.0).abs() < f64::EPSILON);
    assert!((d.within - 1200.0).abs() < f64::EPSILON);
    assert!((d.chapter_dur - 1800.0).abs() < f64::EPSILON);
    assert!((d.chapter_left - 600.0).abs() < f64::EPSILON);
    assert!((d.remaining_left - 2400.0).abs() < f64::EPSILON);
}
