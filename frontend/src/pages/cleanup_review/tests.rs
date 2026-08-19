//! Render + hotkey-mapping tests for the cleanup review page.

use dioxus::prelude::*;
use dioxus_router::{Routable, Router};
use omnibus_shared::{CleanupAction, CleanupKind, SuggestionCard};

use super::{
    action_sentence, kind_title, review_key_action, secondary_label, ReviewKey, SuggestionCardView,
};
use crate::test_support::{render, render_in_vdom};

fn card(kind: CleanupKind, action: CleanupAction) -> SuggestionCard {
    SuggestionCard {
        id: 1,
        kind,
        action,
        decision: omnibus_shared::Decision::Pending,
        primary_name: "Mary Shelley".into(),
        secondary_name: Some("Shelley, Mary W.".into()),
        book_count: 3,
        photo_url: None,
        created_at: 0,
    }
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
fn secondary_label_reads_proposed_for_a_rename_and_merging_away_otherwise() {
    assert_eq!(secondary_label(CleanupAction::Rename), "Proposed");
    assert_eq!(secondary_label(CleanupAction::Merge), "Merging away");
}

#[test]
fn suggestion_card_view_renders_the_names_action_and_book_count() {
    let html = render(rsx! {
        SuggestionCardView { card: card(CleanupKind::Author, CleanupAction::Merge) }
    });
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
    let html = render(rsx! { SuggestionCardView { card: only_one } });
    assert!(html.contains("1 book affected"));
    assert!(!html.contains("Merging away"));
}

#[test]
fn cleanup_review_page_renders_the_loading_state_before_the_queue_arrives() {
    // SSR / first paint: effects never run, so the page must render the
    // loading state — the hydration-parity contract of rule 07.
    let html = render_in_vdom(|| {
        rsx! {
            Router::<ReviewRoute> {}
        }
    });
    assert!(html.contains("Review authors"));
    assert!(html.contains("Accept (Y), reject (N), or skip (Space) each suggestion."));
}
