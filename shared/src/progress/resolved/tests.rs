use super::*;
use crate::progress::{ProgressFormat, ProgressRecord};

fn record(format: ProgressFormat) -> ProgressRecord {
    ProgressRecord {
        book_uuid: "book-1".into(),
        format,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        updated_at: 100,
        client_updated_at: 100,
    }
}

fn detail(format: ProgressFormat) -> ProgressDetail {
    ProgressDetail {
        record: record(format),
        resolved: ResolvedPosition::unknown(),
        total_duration_seconds: None,
        playback_rate: None,
        audio_part: None,
        audio_part_count: None,
    }
}

#[test]
fn resolved_position_unknown_reports_unknown_confidence_and_no_figures() {
    let r = ResolvedPosition::unknown();
    assert_eq!(r.confidence, PositionConfidence::Unknown);
    assert!(r.spine_index.is_none());
    assert!(r.chapter_title.is_none());
    assert!(r.percent_through_book.is_none());
}

#[test]
fn book_progress_empty_has_no_records_and_no_furthest() {
    let bp = BookProgress::empty("book-1".into());
    assert!(bp.records.is_empty());
    assert!(bp.furthest.is_none());
    assert!(bp.furthest_record().is_none());
    assert!(!bp.linked);
}

#[test]
fn furthest_record_selects_the_record_named_by_furthest() {
    let bp = BookProgress {
        book_uuid: "book-1".into(),
        records: vec![detail(ProgressFormat::Epub), detail(ProgressFormat::Audio)],
        furthest: Some(ProgressFormat::Audio),
        linked: true,
        cross_format: None,
    };
    let found = bp.furthest_record().expect("furthest record");
    assert_eq!(found.record.format, ProgressFormat::Audio);
}

#[test]
fn furthest_record_is_none_when_furthest_names_an_absent_format() {
    let bp = BookProgress {
        book_uuid: "book-1".into(),
        records: vec![detail(ProgressFormat::Epub)],
        furthest: Some(ProgressFormat::Audio),
        linked: false,
        cross_format: None,
    };
    assert!(bp.furthest_record().is_none());
}

#[test]
fn position_confidence_serializes_as_snake_case_tokens() {
    let json = serde_json::to_string(&PositionConfidence::Approximate).unwrap();
    assert_eq!(json, "\"approximate\"");
    let parsed: PositionConfidence = serde_json::from_str("\"exact\"").unwrap();
    assert_eq!(parsed, PositionConfidence::Exact);
}

#[test]
fn book_progress_round_trips_through_json() {
    let bp = BookProgress {
        book_uuid: "book-1".into(),
        records: vec![ProgressDetail {
            record: record(ProgressFormat::Audio),
            resolved: ResolvedPosition {
                spine_index: None,
                chapter_title: Some("Chapter Seven".into()),
                chapter_ordinal: Some(7),
                chapters_total: Some(65),
                percent_through_chapter: Some(12.5),
                percent_through_book: Some(87.25),
                confidence: PositionConfidence::Exact,
            },
            total_duration_seconds: Some(78_420.0),
            playback_rate: Some(2.3),
            audio_part: Some(4),
            audio_part_count: Some(4),
        }],
        furthest: Some(ProgressFormat::Audio),
        linked: true,
        cross_format: None,
    };
    let round: BookProgress = serde_json::from_str(&serde_json::to_string(&bp).unwrap()).unwrap();
    assert_eq!(round, bp);
}

#[test]
fn book_progress_decodes_a_payload_carrying_only_its_required_fields() {
    // Every optional field is `skip_serializing_if`, so the minimal shape has
    // to decode — otherwise a book with no positions would fail to parse.
    let json = r#"{"book_uuid":"b","records":[],"linked":false}"#;
    let bp: BookProgress = serde_json::from_str(json).unwrap();
    assert!(bp.records.is_empty());
    assert!(bp.furthest.is_none());
}
