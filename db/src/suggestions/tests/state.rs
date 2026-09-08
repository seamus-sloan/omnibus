//! The pure layer: `decide`'s state machine over pending and resolved
//! markers (debounce, TTL, sticky empty), `is_entry_point`, the
//! case-insensitive author and series comparisons, and
//! `filter_candidates`.

use crate::suggestions::data::{
    decide, SuggestionState, PENDING_DEBOUNCE_SECS, SUGGESTIONS_TTL_SECS,
};
use crate::suggestions::filter::{
    filter_candidates, is_entry_point, is_same_author, is_same_series, Candidate,
};

#[test]
fn decide_enqueues_when_no_marker_exists() {
    let d = decide(None, 1_000);
    assert!(d.enqueue && !d.serve);
}

#[test]
fn decide_does_not_repost_a_recent_pending_marker() {
    let d = decide(
        Some((SuggestionState::Pending, 1_000)),
        1_000 + PENDING_DEBOUNCE_SECS - 1,
    );
    assert!(!d.enqueue && !d.serve);
}

#[test]
fn decide_reposts_a_stale_pending_marker() {
    let d = decide(
        Some((SuggestionState::Pending, 1_000)),
        1_000 + PENDING_DEBOUNCE_SECS + 1,
    );
    assert!(d.enqueue && !d.serve);
}

#[test]
fn decide_serves_fresh_resolved_without_reposting() {
    let d = decide(Some((SuggestionState::Resolved, 1_000)), 1_000 + 10);
    assert!(d.serve && !d.enqueue);
}

#[test]
fn decide_serves_and_refreshes_stale_resolved() {
    let d = decide(
        Some((SuggestionState::Resolved, 1_000)),
        1_000 + SUGGESTIONS_TTL_SECS + 1,
    );
    assert!(d.serve && d.enqueue);
}

#[test]
fn decide_keeps_sticky_empty_quiet_until_ttl() {
    let fresh = decide(Some((SuggestionState::Empty, 1_000)), 1_000 + 10);
    assert!(fresh.serve && !fresh.enqueue);
    let stale = decide(
        Some((SuggestionState::Empty, 1_000)),
        1_000 + SUGGESTIONS_TTL_SECS + 1,
    );
    assert!(stale.serve && stale.enqueue);
}

#[test]
fn is_entry_point_passes_standalones_and_series_starters_only() {
    assert!(is_entry_point(None, None));
    assert!(is_entry_point(Some(""), None));
    assert!(is_entry_point(Some("The Empyrean"), Some(1.0)));
    assert!(is_entry_point(Some("The Empyrean"), Some(0.5)));
    assert!(!is_entry_point(Some("The Empyrean"), Some(2.0)));
    // A series book with unknown position is conservatively excluded.
    assert!(!is_entry_point(Some("The Empyrean"), None));
}

#[test]
fn same_author_and_series_compare_case_insensitively() {
    assert!(is_same_author(
        "rebecca yarros",
        &["Rebecca Yarros".to_string()]
    ));
    assert!(!is_same_author(
        "Brandon Sanderson",
        &["Rebecca Yarros".to_string()]
    ));
    assert!(is_same_series(Some("the empyrean"), Some("The Empyrean")));
    assert!(!is_same_series(Some("The Empyrean"), None));
    assert!(!is_same_series(None, None));
}

fn candidate(id: i64, author: &str, series: Option<&str>, pos: Option<f64>, lc: i64) -> Candidate {
    Candidate {
        hardcover_id: id,
        slug: Some(format!("b{id}")),
        title: format!("Title {id}"),
        author: author.to_string(),
        series: series.map(str::to_string),
        series_position: pos,
        list_count: lc,
        cover_url: None,
    }
}

#[test]
fn filter_candidates_excludes_author_series_nonstarters_and_dedupes() {
    let cands = vec![
        candidate(1, "Author A", None, None, 9), // keep — standalone, diff author
        candidate(1, "Author A", None, None, 9), // dup id — dropped
        candidate(2, "Source Author", None, None, 8), // same author — dropped
        candidate(3, "Author C", Some("Saga"), Some(3.0), 7), // mid-series — dropped
        candidate(4, "Author D", Some("Other"), Some(1.0), 6), // keep — series starter
        candidate(5, "Author E", Some("Trilogy"), None, 5), // unknown position — dropped
        candidate(6, "Author F", None, None, 4), // keep — standalone
    ];
    let source_authors = vec!["Source Author".to_string()];
    let out = filter_candidates(&cands, &source_authors, None, 2);
    // Limit 2, order preserved: ids 1 and 4.
    assert_eq!(
        out.iter().map(|c| c.hardcover_id).collect::<Vec<_>>(),
        vec![1, 4]
    );
}

#[test]
fn filter_candidates_excludes_same_series_as_source() {
    let cands = vec![
        candidate(1, "Author A", Some("The Empyrean"), Some(1.0), 9), // same series — dropped
        candidate(2, "Author B", None, None, 8),                      // keep
    ];
    let out = filter_candidates(&cands, &[], Some("The Empyrean"), 10);
    assert_eq!(
        out.iter().map(|c| c.hardcover_id).collect::<Vec<_>>(),
        vec![2]
    );
}
