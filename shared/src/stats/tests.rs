//! Unit tests for the stats DTO helpers and range serialization.

use super::*;

#[test]
fn total_seconds_sums_reading_and_listening() {
    let s = StatsSummary {
        reading_seconds: 120,
        listening_seconds: 300,
        ..Default::default()
    };
    assert_eq!(s.total_seconds(), 420);
}

#[test]
fn is_empty_is_true_only_without_sessions_or_finishes() {
    assert!(StatsSummary::default().is_empty());
    assert!(!StatsSummary {
        sessions: 1,
        ..Default::default()
    }
    .is_empty());
    assert!(!StatsSummary {
        books_finished: 1,
        ..Default::default()
    }
    .is_empty());
}

#[test]
fn range_defaults_to_year_and_serializes_snake_case() {
    assert_eq!(StatsRange::default(), StatsRange::Year);
    assert_eq!(
        serde_json::to_string(&StatsRange::SixMonths).unwrap(),
        "\"six_months\""
    );
}
