//! Tests for the provider cooldown. Every case uses [`ThrottleTracker::fresh`]
//! and an explicit clock: a tracker sharing the process-wide state, or a test
//! that slept, would make these depend on each other.

use super::*;

const OL: MetadataProvider = MetadataProvider::OpenLibrary;
const GB: MetadataProvider = MetadataProvider::GoogleBooks;

#[test]
fn remaining_is_none_for_a_provider_that_has_not_been_throttled() {
    assert!(ThrottleTracker::fresh().remaining(OL).is_none());
}

#[test]
fn record_starts_the_first_step_of_the_schedule() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, None, now);
    let remaining = tracker.remaining_at(OL, now).expect("cooling down");
    assert_eq!(remaining, Duration::from_secs(DEFAULT_SCHEDULE[0]));
}

#[test]
fn record_escalates_only_while_the_previous_cooldown_is_still_running() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, None, now);
    // A second 429 one second later, well inside the first cooldown.
    let during = now + Duration::from_secs(1);
    tracker.record_at(OL, None, during);
    assert_eq!(
        tracker.remaining_at(OL, during).expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[1])
    );
}

#[test]
fn record_keeps_escalating_across_a_lapsed_cooldown() {
    // The gates mean a provider is never asked *during* a cooldown, so a
    // second 429 can only arrive after one expired. Resetting the level at
    // that moment made every schedule exactly one step long.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, None, now);
    let later = now + Duration::from_secs(DEFAULT_SCHEDULE[0] + 1);
    assert!(
        tracker.remaining_at(OL, later).is_none(),
        "the first step lapsed"
    );
    tracker.record_at(OL, None, later);
    assert_eq!(
        tracker.remaining_at(OL, later).expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[1]),
        "the second refusal takes the second step, not the first again"
    );
}

#[test]
fn clear_is_what_puts_a_provider_back_on_the_first_step() {
    // Evidence the provider is answering again, which a lapse alone is not.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, None, now);
    tracker.record_at(OL, None, now);
    tracker.clear(OL);
    tracker.record_at(OL, None, now);
    assert_eq!(
        tracker.remaining_at(OL, now).expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[0])
    );
}

#[test]
fn record_saturates_at_the_last_step_rather_than_running_off_the_schedule() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    for _ in 0..DEFAULT_SCHEDULE.len() + 3 {
        tracker.record_at(OL, None, now);
    }
    let last = *DEFAULT_SCHEDULE.last().expect("schedule is non-empty");
    assert_eq!(
        tracker.remaining_at(OL, now).expect("cooling down"),
        Duration::from_secs(last)
    );
}

#[test]
fn record_treats_retry_after_as_a_floor_not_a_replacement() {
    // Taken verbatim it made the escalation inert whenever a provider sent
    // one, and let a provider shrink its own cooldown by asking nicely.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, Some(Duration::from_secs(7)), now);
    assert_eq!(
        tracker.remaining_at(OL, now).expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[0]),
        "a shorter Retry-After must not undercut the schedule"
    );
}

#[test]
fn record_honours_a_retry_after_longer_than_the_schedule() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    let long = Duration::from_secs(DEFAULT_SCHEDULE[0] * 3);
    tracker.record_at(OL, Some(long), now);
    assert_eq!(tracker.remaining_at(OL, now).expect("cooling down"), long);
}

#[test]
fn record_never_shortens_a_cooldown_that_is_still_running() {
    // A later refusal carrying a tiny Retry-After must not nearly erase a
    // long cooldown that is still ticking.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    let long = Duration::from_secs(3_600);
    tracker.record_at(OL, Some(long), now);
    tracker.record_at(
        OL,
        Some(Duration::from_secs(1)),
        now + Duration::from_secs(1),
    );
    let left = tracker
        .remaining_at(OL, now + Duration::from_secs(1))
        .expect("cooling down");
    assert!(
        left >= long - Duration::from_secs(2),
        "the running cooldown must survive; got {left:?}"
    );
}

#[test]
fn record_still_escalates_when_the_provider_supplied_its_own_wait() {
    // A provider that keeps saying "one more second" is eventually taken at
    // more than its word: the level advances even though the duration didn't.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, Some(Duration::from_secs(30)), now);
    tracker.record_at(OL, None, now + Duration::from_secs(1));
    assert_eq!(
        tracker
            .remaining_at(OL, now + Duration::from_secs(1))
            .expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[1])
    );
}

#[test]
fn google_books_gets_its_own_longer_schedule() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(GB, None, now);
    assert_eq!(
        tracker.remaining_at(GB, now).expect("cooling down"),
        Duration::from_secs(GOOGLE_BOOKS_SCHEDULE[0])
    );
    assert_ne!(GOOGLE_BOOKS_SCHEDULE[0], DEFAULT_SCHEDULE[0]);
}

#[test]
fn a_cooldown_on_one_provider_leaves_the_others_askable() {
    let tracker = ThrottleTracker::fresh();
    tracker.record(OL, None);
    assert!(tracker.remaining(OL).is_some());
    assert!(tracker.remaining(GB).is_none());
    assert!(tracker.remaining(MetadataProvider::Hardcover).is_none());
}

#[test]
fn clear_ends_the_cooldown_and_resets_the_escalation() {
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, None, now);
    tracker.record_at(OL, None, now);
    tracker.clear(OL);
    assert!(tracker.remaining_at(OL, now).is_none());

    // Back to the first step, not the third.
    tracker.record_at(OL, None, now);
    assert_eq!(
        tracker.remaining_at(OL, now).expect("cooling down"),
        Duration::from_secs(DEFAULT_SCHEDULE[0])
    );
}

#[test]
fn a_cloned_tracker_shares_state_with_its_origin() {
    // The property that lets it ride on a cloned `MetadataLookupConfig`.
    let tracker = ThrottleTracker::fresh();
    let clone = tracker.clone();
    tracker.record(OL, None);
    assert!(clone.remaining(OL).is_some());
}

#[test]
fn fresh_trackers_do_not_share_state() {
    let a = ThrottleTracker::fresh();
    let b = ThrottleTracker::fresh();
    a.record(OL, None);
    assert!(b.remaining(OL).is_none());
}

#[test]
fn retry_after_secs_rounds_up_so_a_wait_never_reads_as_zero() {
    assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
    assert_eq!(retry_after_secs(Duration::from_millis(1500)), 2);
    assert_eq!(retry_after_secs(Duration::from_secs(30)), 30);
}

#[test]
fn parse_retry_after_reads_delta_seconds() {
    assert_eq!(
        parse_retry_after(Some("120")),
        Some(Duration::from_secs(120))
    );
    assert_eq!(
        parse_retry_after(Some("  90 ")),
        Some(Duration::from_secs(90))
    );
}

#[test]
fn parse_retry_after_reads_an_http_date() {
    let future = std::time::SystemTime::now() + Duration::from_secs(600);
    let header = httpdate::fmt_http_date(future);
    let parsed = parse_retry_after(Some(&header)).expect("a future date parses");
    // Whole-second formatting means the value lands just under ten minutes.
    assert!(
        parsed <= Duration::from_secs(600) && parsed >= Duration::from_secs(598),
        "got {parsed:?}"
    );
}

#[test]
fn parse_retry_after_ignores_values_that_say_nothing() {
    assert_eq!(parse_retry_after(None), None);
    assert_eq!(parse_retry_after(Some("")), None);
    assert_eq!(parse_retry_after(Some("0")), None);
    assert_eq!(parse_retry_after(Some("-5")), None);
    assert_eq!(parse_retry_after(Some("soon")), None);
    // An HTTP-date already in the past is not a wait.
    let past = std::time::SystemTime::now() - Duration::from_secs(600);
    assert_eq!(
        parse_retry_after(Some(&httpdate::fmt_http_date(past))),
        None
    );
}

#[test]
fn record_caps_an_absurd_retry_after_rather_than_panicking() {
    // `Instant + Duration` panics on overflow, so a provider could otherwise
    // take the process down with one header.
    let tracker = ThrottleTracker::fresh();
    let now = Instant::now();
    tracker.record_at(OL, Some(Duration::from_secs(u64::MAX / 2)), now);
    assert_eq!(
        tracker.remaining_at(OL, now).expect("cooling down"),
        MAX_COOLDOWN
    );
}

#[test]
fn parse_retry_after_accepts_a_huge_value_and_leaves_the_cap_to_record() {
    // Parsing reports what the provider said; bounding it is `record`'s job,
    // so the two concerns stay separable.
    assert_eq!(
        parse_retry_after(Some("999999999")),
        Some(Duration::from_secs(999_999_999))
    );
}
