//! Tests for [`super::compute_suggestions`]: pool/query/focus filtering.

use super::*;

#[test]
fn compute_suggestions_returns_empty_when_pool_is_empty() {
    let result = compute_suggestions(&[], &[], "", true, false);
    assert!(result.is_empty());
}

#[test]
fn compute_suggestions_filters_already_chosen_values() {
    let pool = vec![
        SuggestionItem::new("Ada", 1),
        SuggestionItem::new("Mira", 2),
    ];
    let result = compute_suggestions(&pool, &["Ada".to_string()], "", true, false);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "Mira");
}

#[test]
fn compute_suggestions_returns_empty_when_unfocused_and_query_empty() {
    let pool = vec![SuggestionItem::new("Ada", 1)];
    let result = compute_suggestions(&pool, &[], "", false, false);
    assert!(result.is_empty());
}

#[test]
fn compute_suggestions_returns_empty_when_suppress_open_is_true() {
    let pool = vec![SuggestionItem::new("Ada", 1)];
    let result = compute_suggestions(&pool, &[], "", true, true);
    assert!(result.is_empty());
}
