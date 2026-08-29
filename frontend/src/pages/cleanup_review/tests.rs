//! Render + hotkey-mapping tests for the cleanup review page.

use dioxus::prelude::*;
use dioxus_router::{Routable, Router};
use omnibus_shared::{CleanupAction, CleanupKind, SuggestionCard};

use super::card::{action_sentence, initial_value, is_editable, SuggestionCardView};
use super::frame::{kind_label, kind_title, CleanupProgress};
use super::{edited_value, review_key_action, ReviewKey};
use crate::test_support::{render, render_in_vdom};

fn card(kind: CleanupKind, action: CleanupAction) -> SuggestionCard {
    SuggestionCard {
        id: 1,
        kind,
        action,
        decision: omnibus_shared::Decision::Pending,
        primary_name: "Mary Shelley".into(),
        secondary_name: Some("Shelley, Mary W.".into()),
        source_names: vec!["Shelley, Mary W.".into()],
        proposed_parts: Vec::new(),
        book_count: 3,
        photo_url: None,
        created_at: 0,
    }
}

fn rename_card() -> SuggestionCard {
    SuggestionCard {
        primary_name: "Shelley, Mary - Frankenstein".into(),
        secondary_name: Some("Frankenstein".into()),
        book_count: 1,
        ..card(CleanupKind::BookTitle, CleanupAction::Rename)
    }
}

/// Owns the signals the card view takes as props, seeded the way the page
/// seeds them, so a test can render one card by value.
#[component]
fn CardHarness(card: SuggestionCard) -> Element {
    let draft = use_signal(|| initial_value(&card));
    let typing = use_signal(|| false);
    rsx! {
        SuggestionCardView { card, draft, typing, on_commit: move |()| {} }
    }
}

fn render_card(card: SuggestionCard) -> String {
    render(rsx! { CardHarness { card } })
}

// The page calls `use_navigator`-free hooks but mounts inside the router in
// production; the card view itself is hookless, so it renders directly.
#[derive(Clone, Debug, PartialEq, Routable)]
enum ReviewRoute {
    #[route("/")]
    ReviewHost {},
}

#[component]
fn ReviewHost() -> Element {
    rsx! {
        super::CleanupReviewPage { kind: "author".to_string() }
    }
}

#[test]
fn review_key_action_maps_y_n_and_space_case_insensitively() {
    assert_eq!(
        review_key_action(&Key::Character("y".into())),
        Some(ReviewKey::Accept)
    );
    assert_eq!(
        review_key_action(&Key::Character("Y".into())),
        Some(ReviewKey::Accept)
    );
    assert_eq!(
        review_key_action(&Key::Character("n".into())),
        Some(ReviewKey::Reject)
    );
    assert_eq!(
        review_key_action(&Key::Character(" ".into())),
        Some(ReviewKey::Skip)
    );
}

#[test]
fn review_key_action_ignores_keys_that_are_not_review_hotkeys() {
    assert_eq!(review_key_action(&Key::Character("q".into())), None);
    assert_eq!(review_key_action(&Key::Enter), None);
}

#[test]
fn kind_title_names_each_kind_and_falls_back_for_an_unknown_slug() {
    assert_eq!(kind_title(Some(CleanupKind::Author)), "Review authors");
    assert_eq!(kind_title(Some(CleanupKind::Tag)), "Review tags");
    assert_eq!(
        kind_title(Some(CleanupKind::BookTitle)),
        "Review book titles"
    );
    assert_eq!(kind_title(None), "Review library cleanup");
}

#[test]
fn kind_label_names_every_kind_chip() {
    assert_eq!(kind_label(CleanupKind::Author), "Authors");
    assert_eq!(kind_label(CleanupKind::Series), "Series");
    assert_eq!(kind_label(CleanupKind::Tag), "Tags");
    assert_eq!(kind_label(CleanupKind::BookTitle), "Book titles");
}

#[test]
fn action_sentence_describes_each_supported_kind_and_action_pair() {
    assert_eq!(
        action_sentence(CleanupKind::Author, CleanupAction::Merge),
        "Merge these authors into one."
    );
    assert_eq!(
        action_sentence(CleanupKind::Tag, CleanupAction::Split),
        "Split this tag into its parts."
    );
    assert_eq!(
        action_sentence(CleanupKind::BookTitle, CleanupAction::Rename),
        "Adopt the normalized title."
    );
    // An unrepresented pair still renders a sentence rather than blank space.
    assert_eq!(
        action_sentence(CleanupKind::Series, CleanupAction::Delete),
        "Apply this cleanup."
    );
}

#[test]
fn is_editable_is_true_only_for_a_book_title_rename() {
    // The apply path is what decides this: only `apply_book_title_override`
    // takes an arbitrary value, so only that card may offer an edit.
    assert!(is_editable(&rename_card()));
    assert!(!is_editable(&card(
        CleanupKind::Author,
        CleanupAction::Merge
    )));
    assert!(!is_editable(&card(CleanupKind::Tag, CleanupAction::Split)));
}

#[test]
fn initial_value_is_the_proposal_for_a_rename_and_the_survivor_otherwise() {
    assert_eq!(initial_value(&rename_card()), "Frankenstein");
    assert_eq!(
        initial_value(&card(CleanupKind::Author, CleanupAction::Merge)),
        "Mary Shelley"
    );
}

#[test]
fn edited_value_is_none_when_the_draft_still_reads_as_detected() {
    assert_eq!(edited_value(&rename_card(), "Frankenstein"), None);
}

#[test]
fn edited_value_carries_the_reviewers_wording_when_they_typed_over_it() {
    assert_eq!(
        edited_value(&rename_card(), "Frankenstein; or, The Modern Prometheus"),
        Some("Frankenstein; or, The Modern Prometheus".to_string())
    );
}

#[test]
fn edited_value_stays_none_for_a_kind_whose_apply_path_cannot_carry_one() {
    // Sending it anyway would be refused by the db layer; not sending it is
    // what keeps the button honest about what accepting will write.
    let merge = card(CleanupKind::Author, CleanupAction::Merge);
    assert_eq!(edited_value(&merge, "Someone Else"), None);
}

#[test]
fn suggestion_card_view_renders_the_action_names_and_book_count() {
    let html = render_card(card(CleanupKind::Author, CleanupAction::Merge));
    assert!(html.contains("Merge these authors into one."));
    assert!(html.contains("Mary Shelley"));
    assert!(html.contains("Shelley, Mary W."));
    assert!(html.contains("3 books affected"));
}

#[test]
fn suggestion_card_view_singularizes_a_one_book_suggestion() {
    let mut only_one = card(CleanupKind::Tag, CleanupAction::Split);
    only_one.book_count = 1;
    only_one.secondary_name = None;
    let html = render_card(only_one);
    assert!(html.contains("1 book affected"));
}

#[test]
fn suggestion_card_view_offers_an_input_only_where_the_value_is_editable() {
    let editable = render_card(rename_card());
    assert!(editable.contains("cleanup-proposed-input"));
    let fixed = render_card(card(CleanupKind::Author, CleanupAction::Merge));
    assert!(!fixed.contains("cleanup-proposed-input"));
    assert!(fixed.contains("crx-proposed-static"));
}

#[test]
fn cleanup_progress_marks_passed_current_and_upcoming_dots() {
    let html = render(rsx! { CleanupProgress { index: 1usize, total: 3usize } });
    assert!(html.contains("2 of 3"));
    assert!(html.contains("crx-dot done"));
    assert!(html.contains("crx-dot on"));
}

#[test]
fn cleanup_review_page_renders_its_frame_before_the_queue_arrives() {
    // SSR / first paint: effects never run and `CurrentUser` has not resolved,
    // so only the chrome outside the admin gate is on the page. It has to be
    // *there* — rule 07's parity contract is that the first client render
    // produces this same markup to hydrate onto.
    let html = render_in_vdom(|| {
        rsx! {
            Router::<ReviewRoute> {}
        }
    });
    assert!(html.contains("Review authors"), "the heading");
    assert!(html.contains("cleanup-kindline"), "the kind chips");
    assert!(html.contains("cleanup-review"), "the focusable column");
    // The queue body is gated on an admin the first paint doesn't know about.
    assert!(html.contains("cleanup-review-forbidden"));
}

#[test]
fn suggestion_card_view_names_every_record_a_merge_folds_away() {
    let mut three_way = card(CleanupKind::Author, CleanupAction::Merge);
    three_way.secondary_name = None;
    three_way.source_names = vec!["Shelley, Mary W.".into(), "M. Shelley".into()];
    let html = render_card(three_way);
    assert!(html.contains("Shelley, Mary W."));
    assert!(html.contains("M. Shelley"));
}

#[test]
fn suggestion_card_view_shows_the_atoms_a_split_would_write() {
    // Not the scanned tag twice: the proposed side has to be the parts.
    let mut split = card(CleanupKind::Tag, CleanupAction::Split);
    split.primary_name = "Fiction; Poland".into();
    split.secondary_name = None;
    split.source_names = Vec::new();
    split.proposed_parts = vec!["Fiction".into(), "Poland".into()];
    let html = render_card(split);
    assert!(html.contains("crx-part"));
    assert!(html.contains(">Fiction<"));
    assert!(html.contains(">Poland<"));
}
