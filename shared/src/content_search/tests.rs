use super::*;

#[test]
fn spoiler_filter_defaults_to_annotate() {
    assert_eq!(SpoilerFilter::default(), SpoilerFilter::Annotate);
}

#[test]
fn spoiler_filter_round_trips_snake_case_tokens() {
    assert_eq!(
        serde_json::to_string(&SpoilerFilter::Exclude).unwrap(),
        "\"exclude\""
    );
    let parsed: SpoilerFilter = serde_json::from_str("\"none\"").unwrap();
    assert_eq!(parsed, SpoilerFilter::None);
}

#[test]
fn results_omit_the_optional_fields_when_absent() {
    let json = serde_json::to_string(&ContentSearchResults::default()).unwrap();
    assert_eq!(json, r#"{"hits":[]}"#);
}

#[test]
fn a_hit_predating_the_enrichment_still_decodes() {
    let json = r#"{"book_uuid":"b","spine_index":3,"title":"T","snippet":"x"}"#;
    let hit: ContentSearchHit = serde_json::from_str(json).unwrap();
    assert!(hit.chapter_title.is_none());
    assert!(hit.ahead_of_reader.is_none());
    assert!(hit.position_delta_percent.is_none());
}

#[test]
fn results_carry_a_zero_withheld_count_so_exclude_mode_can_say_nothing_was_held() {
    let results = ContentSearchResults {
        hits: Vec::new(),
        withheld_ahead: Some(0),
        hint: None,
    };
    let json = serde_json::to_string(&results).unwrap();
    assert!(json.contains(r#""withheld_ahead":0"#));
}
