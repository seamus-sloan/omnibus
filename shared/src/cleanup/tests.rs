//! Serde round-trip and token-parsing tests for the Library Cleanup wire types.

use super::*;

#[test]
fn cleanup_kind_round_trips_through_str_tokens() {
    for k in [
        CleanupKind::Author,
        CleanupKind::Series,
        CleanupKind::Tag,
        CleanupKind::BookTitle,
    ] {
        assert_eq!(CleanupKind::from_str(k.as_str()), Some(k));
    }
}

#[test]
fn cleanup_kind_from_str_rejects_unknown_token() {
    assert_eq!(CleanupKind::from_str("publisher"), None);
}

#[test]
fn cleanup_kind_serializes_as_snake_case_json_string() {
    let json = serde_json::to_string(&CleanupKind::BookTitle).unwrap();
    assert_eq!(json, "\"book_title\"");
    let back: CleanupKind = serde_json::from_str(&json).unwrap();
    assert_eq!(back, CleanupKind::BookTitle);
}

#[test]
fn cleanup_action_round_trips_through_str_tokens() {
    for a in [
        CleanupAction::Merge,
        CleanupAction::Split,
        CleanupAction::Rename,
        CleanupAction::Delete,
    ] {
        assert_eq!(CleanupAction::from_str(a.as_str()), Some(a));
    }
}

#[test]
fn cleanup_action_from_str_rejects_unknown_token() {
    assert_eq!(CleanupAction::from_str("archive"), None);
}

#[test]
fn cleanup_action_serializes_as_snake_case_json_string() {
    let json = serde_json::to_string(&CleanupAction::Merge).unwrap();
    assert_eq!(json, "\"merge\"");
    let back: CleanupAction = serde_json::from_str(&json).unwrap();
    assert_eq!(back, CleanupAction::Merge);
}

#[test]
fn decision_round_trips_through_str_tokens() {
    for d in [Decision::Pending, Decision::Accepted, Decision::Rejected] {
        assert_eq!(Decision::from_str(d.as_str()), Some(d));
    }
}

#[test]
fn decision_from_str_rejects_unknown_token() {
    assert_eq!(Decision::from_str("maybe"), None);
}

#[test]
fn decision_defaults_to_pending() {
    assert_eq!(Decision::default(), Decision::Pending);
}

#[test]
fn cleanup_counts_round_trips_through_json() {
    let counts = CleanupCounts {
        pending: 3,
        accepted: 1,
        rejected: 2,
    };
    let json = serde_json::to_string(&counts).unwrap();
    let back: CleanupCounts = serde_json::from_str(&json).unwrap();
    assert_eq!(back, counts);
}

#[test]
fn suggestion_card_round_trips_through_json_with_optional_fields_present_and_absent() {
    let with_optionals = SuggestionCard {
        id: 1,
        kind: CleanupKind::Author,
        action: CleanupAction::Merge,
        decision: Decision::Pending,
        primary_name: "Ursula K. Le Guin".into(),
        secondary_name: Some("Ursula Le Guin".into()),
        source_names: vec!["Ursula Le Guin".into()],
        proposed_parts: Vec::new(),
        book_count: 12,
        photo_url: Some("/api/authors/1/photo".into()),
        created_at: 1_700_000_000,
    };
    let json = serde_json::to_string(&with_optionals).unwrap();
    let back: SuggestionCard = serde_json::from_str(&json).unwrap();
    assert_eq!(back, with_optionals);

    let without_optionals = SuggestionCard {
        secondary_name: None,
        photo_url: None,
        ..with_optionals
    };
    let json = serde_json::to_string(&without_optionals).unwrap();
    let back: SuggestionCard = serde_json::from_str(&json).unwrap();
    assert_eq!(back, without_optionals);
}

#[test]
fn suggestion_card_deserializes_a_payload_written_before_the_list_fields_existed() {
    // `source_names` and `proposed_parts` carry `#[serde(default)]` so a card
    // from a server that predates them still decodes — the two lists are
    // additive, and a client on an older build must not fail the whole queue
    // over their absence.
    let json = r#"{
        "id": 7, "kind": "tag", "action": "split", "decision": "pending",
        "primary_name": "a;b", "secondary_name": null,
        "book_count": 2, "photo_url": null, "created_at": 1700000000
    }"#;
    let card: SuggestionCard = serde_json::from_str(json).unwrap();
    assert!(card.source_names.is_empty());
    assert!(card.proposed_parts.is_empty());
}
