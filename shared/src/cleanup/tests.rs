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
        CleanupAction::Alias,
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
fn ignored_author_round_trips_through_json() {
    let entry = IgnoredAuthor {
        name: "Weir, Andy".into(),
        ignored_at: 1_700_000_000,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: IgnoredAuthor = serde_json::from_str(&json).unwrap();
    assert_eq!(back, entry);
}
