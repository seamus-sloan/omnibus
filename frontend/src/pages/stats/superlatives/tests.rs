//! Tests for the superlatives card's row assembly and its omit-rather-than-
//! empty behaviour.

use omnibus_shared::{DayActivity, Superlatives};

use super::*;

fn book(title: &str, value: i64) -> BookSuperlative {
    BookSuperlative {
        book_uuid: title.to_string(),
        title: title.to_string(),
        author: None,
        value,
    }
}

fn ranked(name: &str, seconds: i64) -> RankedEntity {
    RankedEntity {
        name: name.to_string(),
        seconds,
    }
}

#[test]
fn pretty_day_renders_a_readable_date_and_passes_garbage_through() {
    assert_eq!(pretty_day("2023-11-14"), "14 Nov 2023");
    assert_eq!(pretty_day("not-a-day"), "not-a-day");
}

#[test]
fn pages_and_days_details_singularize() {
    assert_eq!(pages_detail(1), "1 page");
    assert_eq!(pages_detail(412), "412 pages");
    assert_eq!(days_detail(1), "in a day");
    assert_eq!(days_detail(3), "in 3 days");
}

#[test]
fn build_rows_omits_every_superlative_the_server_left_out() {
    // The all-`Option` payload is the point: an absent superlative costs its
    // row, not an em-dash.
    assert!(build_rows(&StatsSummary::default()).is_empty());
}

#[test]
fn build_rows_renders_each_superlative_the_window_supports() {
    let summary = StatsSummary {
        superlatives: Superlatives {
            longest_book: Some(book("Doorstopper", 900)),
            shortest_book: Some(book("Novella", 90)),
            biggest_day: Some(DayActivity {
                day: "2023-11-14".to_string(),
                seconds: 7200,
            }),
            longest_sit: Some(book("Marathon", 5400)),
            fastest_read: Some(book("Sprint", 3)),
        },
        busiest_week_start: Some("2023-11-13".to_string()),
        busiest_week_seconds: 14_400,
        top_authors: vec![ranked("Ursula K. Le Guin", 3600)],
        top_tags: vec![ranked("Science Fiction", 1800)],
        ..Default::default()
    };

    let rows = build_rows(&summary);

    let labels: Vec<&str> = rows.iter().map(|r| r.label).collect();
    assert_eq!(
        labels,
        [
            "Longest book",
            "Shortest book",
            "Fastest read",
            "Longest sitting",
            "Biggest day",
            "Busiest week",
            "Most-read author",
            "Most-read subject",
        ]
    );
    assert_eq!(rows[0].detail, "900 pages");
    assert_eq!(rows[2].detail, "in 3 days");
    assert_eq!(rows[3].detail, "1 h 30 m");
    assert_eq!(rows[4].headline, "14 Nov 2023");
    assert_eq!(rows[5].headline, "Week of 13 Nov 2023");
}

#[test]
fn busiest_week_is_omitted_when_the_payload_carries_no_seconds_for_it() {
    // The field is on every payload and zeroed for an empty window; rendering
    // it unconditionally would claim a busiest week that never happened.
    let summary = StatsSummary {
        busiest_week_start: Some("2023-11-13".to_string()),
        busiest_week_seconds: 0,
        ..Default::default()
    };

    assert!(build_rows(&summary).is_empty());
}

#[test]
fn ranked_rows_are_omitted_for_an_empty_or_timeless_leader() {
    let summary = StatsSummary {
        top_authors: vec![ranked("Nobody", 0)],
        ..Default::default()
    };

    assert!(build_rows(&summary).is_empty());
}

#[test]
fn fastest_read_note_states_the_floor_rather_than_a_hardcoded_number() {
    let note = fastest_read_note();
    assert!(
        note.contains(&format_active_time(FASTEST_READ_MIN_SECS)),
        "{note}"
    );
}

#[cfg(feature = "server")]
#[test]
fn superlatives_card_renders_its_rows_and_the_fastest_read_caveat() {
    let summary = StatsSummary {
        superlatives: Superlatives {
            longest_book: Some(book("Doorstopper", 900)),
            fastest_read: Some(book("Sprint", 3)),
            ..Default::default()
        },
        ..Default::default()
    };

    let html = crate::test_support::render(rsx! { StandoutsGrid { summary } });

    assert!(html.contains("stats-superlatives"), "{html}");
    assert!(html.contains("Doorstopper"), "{html}");
    assert!(html.contains("900 pages"), "{html}");
    // The floor is part of the claim, not a footnote to skip (AC5).
    assert!(html.contains("stats-superlatives-note"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn superlatives_card_renders_nothing_at_all_for_a_bare_window() {
    let html = crate::test_support::render(rsx! {
        StandoutsGrid { summary: StatsSummary::default() }
    });

    assert!(!html.contains("stats-superlatives"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn superlatives_card_omits_the_caveat_when_it_reports_no_fastest_read() {
    let summary = StatsSummary {
        superlatives: Superlatives {
            longest_book: Some(book("Doorstopper", 900)),
            ..Default::default()
        },
        ..Default::default()
    };

    let html = crate::test_support::render(rsx! { StandoutsGrid { summary } });

    assert!(html.contains("stats-superlatives"), "{html}");
    assert!(!html.contains("stats-superlatives-note"), "{html}");
}
