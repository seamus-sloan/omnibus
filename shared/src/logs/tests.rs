//! Unit tests for the admin log-viewer wire types (`per_page` clamping and
//! serde shape).

use super::*;

#[test]
fn effective_per_page_defaults_when_zero() {
    let q = LogQuery::default();
    assert_eq!(q.effective_per_page(), LOG_PER_PAGE_DEFAULT);
}

#[test]
fn effective_per_page_clamps_to_max() {
    let q = LogQuery {
        per_page: LOG_PER_PAGE_MAX + 1_000,
        ..Default::default()
    };
    assert_eq!(q.effective_per_page(), LOG_PER_PAGE_MAX);
}

#[test]
fn effective_per_page_passes_through_in_range() {
    let q = LogQuery {
        per_page: 25,
        ..Default::default()
    };
    assert_eq!(q.effective_per_page(), 25);
}

#[test]
fn log_query_omits_empty_filters_from_json() {
    let json = serde_json::to_string(&LogQuery::default()).unwrap();
    // Only the always-present page/per_page numeric fields survive.
    assert_eq!(json, r#"{"page":0,"per_page":0}"#);
}

#[test]
fn log_record_round_trips_through_json() {
    let rec = LogRecord {
        timestamp: "2026-07-19T00:00:25.011938Z".into(),
        level: "INFO".into(),
        target: "omnibus::backend::covers".into(),
        message: "finished processing request".into(),
        fields: Some(r#"{"status":200}"#.into()),
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert_eq!(serde_json::from_str::<LogRecord>(&json).unwrap(), rec);
}
