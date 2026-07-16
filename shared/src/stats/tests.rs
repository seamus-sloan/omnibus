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
fn range_defaults_to_month_and_serializes_snake_case() {
    assert_eq!(StatsRange::default(), StatsRange::Month);
    assert_eq!(
        serde_json::to_string(&StatsRange::AllTime).unwrap(),
        "\"all_time\""
    );
    assert_eq!(
        serde_json::from_str::<StatsRange>("\"week\"").unwrap(),
        StatsRange::Week
    );
}

#[test]
fn avg_stars_defaults_to_none_when_absent_from_the_wire() {
    // Older payloads predate the field; serde(default) keeps them parseable.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.avg_stars, None);
}

#[test]
fn books_per_month_defaults_to_empty_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars — the monthly trend chart is a
    // newer field, so a pre-existing payload without it must still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert!(s.books_per_month.is_empty());
}

#[test]
fn month_count_round_trips_through_json() {
    let m = MonthCount {
        month: "2026-07".to_string(),
        books: 3,
    };
    let wire = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<MonthCount>(&wire).unwrap(), m);
}

#[test]
fn previous_and_trend_fields_default_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/books_per_month — the drill-in
    // fields are newer, so a pre-existing payload without them must still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.previous, PeriodComparison::default());
    assert!(s.listening_daily.is_empty());
    assert!(s.rating_monthly.is_empty());
}

#[test]
fn pages_read_defaults_to_none_when_absent_from_the_wire() {
    // Same older-payload contract as avg_stars/books_per_month — the Pages
    // tile field is newer, so a pre-existing payload without it must
    // still parse.
    let s: StatsSummary = serde_json::from_str(
        r#"{"range":"month","reading_seconds":0,"listening_seconds":0,"sessions":0,
            "active_days":0,"longest_streak_days":0,"busiest_week_start":null,
            "busiest_week_seconds":0,"books_finished":0,"heatmap":[],
            "top_authors":[],"top_tags":[],"finished_books":[]}"#,
    )
    .unwrap();
    assert_eq!(s.pages_read, None);
}

#[test]
fn finished_book_cover_url_and_rating_default_to_none_when_absent() {
    let b: FinishedBook =
        serde_json::from_str(r#"{"book_uuid":"u1","title":"Dune","author":null,"finished_at":0}"#)
            .unwrap();
    assert_eq!(b.cover_url, None);
    assert_eq!(b.rating, None);
}

#[test]
fn trend_point_round_trips_through_json() {
    let p = TrendPoint {
        label: "2026-07".to_string(),
        value: 4.25,
    };
    let wire = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<TrendPoint>(&wire).unwrap(), p);
}

#[test]
fn as_query_matches_the_serde_wire_name() {
    for range in StatsRange::ALL {
        let wire = serde_json::to_string(&range).unwrap();
        assert_eq!(wire, format!("\"{}\"", range.as_query()));
    }
}

#[test]
fn range_labels_render_all_time_as_lifetime() {
    let labels: Vec<&str> = StatsRange::ALL.iter().map(|r| r.label()).collect();
    assert_eq!(labels, ["Week", "Month", "Year", "Lifetime"]);
}
