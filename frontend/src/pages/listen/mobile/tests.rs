//! Tests for the mobile player's per-render display derivation — pins the
//! scrubber row's one-basis contract at the `derive_player_state` boundary.

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

// Issue #2246: every readout is book time, so a speed change moves none of
// them — the transport can't disagree with the chapter list, and elapsed
// can't run backwards when the listener speeds up.
#[test]
fn derive_player_state_reads_book_time_at_every_playback_rate() {
    // 20 book-minutes into the first of two 30-minute chapters.
    let at = |rate: f64| {
        derive_player_state(
            &two_chapter_view(),
            1200.0,
            3600.0,
            0,
            rate,
            SleepState::Off,
            None,
        )
    };
    let d = at(2.0);
    assert!((d.elapsed_book - 1200.0).abs() < f64::EPSILON);
    assert!((d.within - 1200.0).abs() < f64::EPSILON);
    // The chapter total matches the chapter list's own `duration_seconds`.
    assert!((d.chapter_dur - 1800.0).abs() < f64::EPSILON);
    assert!((d.chapter_left - 600.0).abs() < f64::EPSILON);
    assert!((d.remaining_book - 2400.0).abs() < f64::EPSILON);
    assert!((d.within + d.chapter_left - d.chapter_dur).abs() < f64::EPSILON);
    assert!((d.elapsed_book + d.remaining_book - 3600.0).abs() < f64::EPSILON);
    // The seek coordinates share that basis — value/max on the range input.
    assert!((d.effective - 1200.0).abs() < f64::EPSILON);
    assert!((d.scrub_max - 3600.0).abs() < f64::EPSILON);

    // Speeding up never moves a readout, least of all backwards.
    for rate in [0.5, 1.0, 1.5, 3.0] {
        let other = at(rate);
        assert!((other.elapsed_book - d.elapsed_book).abs() < f64::EPSILON);
        assert!((other.chapter_dur - d.chapter_dur).abs() < f64::EPSILON);
        assert!((other.remaining_book - d.remaining_book).abs() < f64::EPSILON);
    }
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
    assert!((d.remaining_book - 2400.0).abs() < f64::EPSILON);
}
