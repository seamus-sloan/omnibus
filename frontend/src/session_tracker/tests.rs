//! Tests for the [`SessionTracker`] segment state machine — every pub
//! transition gets a happy path, plus the short-segment and backwards-clock
//! boundaries that guard the stats aggregates from junk rows.

use omnibus_shared::ProgressFormat;

use super::{SessionTracker, MIN_SESSION_SECS};

#[test]
fn start_from_idle_begins_a_segment_and_reports_nothing() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    // The segment is live: stopping later yields the report.
    let r = t.stop(100 + MIN_SESSION_SECS).expect("segment should emit");
    assert_eq!(r.book_uuid, "book-a");
    assert_eq!(r.started_at, 100);
    assert_eq!(r.ended_at, 100 + MIN_SESSION_SECS);
    assert_eq!(r.progress_units, MIN_SESSION_SECS);
    assert_eq!(r.format, ProgressFormat::Epub);
    assert!(r.device_id.is_none());
}

#[test]
fn start_on_the_same_book_is_a_noop_that_keeps_the_original_segment() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    assert!(t.start("book-a", 130).is_none());
    let r = t.stop(160).expect("segment should emit");
    // The original start survives the redundant re-start.
    assert_eq!(r.started_at, 100);
    assert_eq!(r.progress_units, 60);
}

#[test]
fn start_on_a_different_book_flushes_the_old_segment_and_starts_the_new() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    let flushed = t.start("book-b", 160).expect("old segment should emit");
    assert_eq!(flushed.book_uuid, "book-a");
    assert_eq!(flushed.progress_units, 60);
    let r = t.stop(220).expect("new segment should emit");
    assert_eq!(r.book_uuid, "book-b");
    assert_eq!(r.started_at, 160);
}

#[test]
fn stop_discards_segments_shorter_than_the_minimum() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    assert!(t.stop(100 + MIN_SESSION_SECS - 1).is_none());
    // Fully idle afterwards: another stop emits nothing.
    assert!(t.stop(500).is_none());
}

#[test]
fn stop_discards_segments_when_the_clock_went_backwards() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    assert!(t.stop(50).is_none());
}

#[test]
fn stop_when_idle_reports_nothing() {
    let mut t = SessionTracker::new(ProgressFormat::Audio);
    assert!(t.stop(100).is_none());
}

#[test]
fn suspend_emits_the_segment_and_resume_restarts_it_for_the_same_book() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    let r = t.suspend(160).expect("suspend should emit");
    assert_eq!(r.book_uuid, "book-a");
    assert_eq!(r.progress_units, 60);
    // Paused time is not counted: resume restarts the clock at `now`.
    t.resume(400);
    let r = t.stop(460).expect("resumed segment should emit");
    assert_eq!(r.book_uuid, "book-a");
    assert_eq!(r.started_at, 400);
    assert_eq!(r.progress_units, 60);
}

#[test]
fn suspend_when_idle_reports_nothing_and_resume_stays_idle() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.suspend(100).is_none());
    t.resume(200);
    assert!(t.stop(400).is_none());
}

#[test]
fn resume_while_active_keeps_the_running_segment() {
    let mut t = SessionTracker::new(ProgressFormat::Epub);
    assert!(t.start("book-a", 100).is_none());
    t.resume(130);
    let r = t.stop(160).expect("segment should emit");
    assert_eq!(r.started_at, 100);
}

#[test]
fn rollover_emits_the_segment_and_restarts_it_at_now() {
    let mut t = SessionTracker::new(ProgressFormat::Audio);
    assert!(t.start("book-a", 100).is_none());
    let first = t.rollover(160).expect("first interval should emit");
    assert_eq!(first.started_at, 100);
    assert_eq!(first.ended_at, 160);
    assert_eq!(first.progress_units, 60);
    // Segment continues from the rollover point without a gap.
    let second = t.stop(220).expect("tail should emit");
    assert_eq!(second.started_at, 160);
    assert_eq!(second.progress_units, 60);
}

#[test]
fn rollover_keeps_short_segments_accruing_unreported() {
    let mut t = SessionTracker::new(ProgressFormat::Audio);
    assert!(t.start("book-a", 100).is_none());
    assert!(t.rollover(100 + MIN_SESSION_SECS - 1).is_none());
    // The original start is preserved — nothing was emitted or reset.
    let r = t.stop(160).expect("segment should emit");
    assert_eq!(r.started_at, 100);
    assert_eq!(r.progress_units, 60);
}

#[test]
fn rollover_when_idle_reports_nothing() {
    let mut t = SessionTracker::new(ProgressFormat::Audio);
    assert!(t.rollover(100).is_none());
}
