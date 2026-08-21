//! Render tests for the delete-author modal's two modes: plain delete
//! (blocklists the name) vs. "this is a duplicate of…" (merges into a
//! picked canonical author, no blocklist write).

use dioxus::prelude::*;
use dioxus_router::Routable;

use super::{AuthorDeleteModal, AuthorDeleteState};
use crate::components::AuthorPick;
use crate::test_support::render_in_vdom;

fn modal_state(duplicate_of: Option<AuthorPick>) -> AuthorDeleteState {
    AuthorDeleteState {
        show_confirm: use_signal(|| true),
        deleting: use_signal(|| false),
        delete_error: use_signal(|| None),
        duplicate_of: use_signal(move || duplicate_of.clone()),
    }
}

fn modal(state: AuthorDeleteState) -> Element {
    rsx! {
        AuthorDeleteModal {
            author_id: 7,
            author_name: "Weir, Andy".to_string(),
            book_count: 1,
            server_url: String::new(),
            state,
        }
    }
}

// The modal calls `use_navigator`, so each mode mounts behind its own
// one-route router host (a `Routable` host component takes no props).
#[derive(Clone, Debug, PartialEq, Routable)]
enum PlainRoute {
    #[route("/")]
    PlainHost {},
}

#[component]
fn PlainHost() -> Element {
    let state = modal_state(None);
    modal(state)
}

#[derive(Clone, Debug, PartialEq, Routable)]
enum PickedRoute {
    #[route("/")]
    PickedHost {},
}

#[component]
fn PickedHost() -> Element {
    let state = modal_state(Some(AuthorPick {
        id: 42,
        name: "Andy Weir".into(),
    }));
    modal(state)
}

#[test]
fn delete_modal_defaults_to_the_blocklisting_delete_with_a_duplicate_picker() {
    let html = render_in_vdom(|| rsx! { dioxus_router::Router::<PlainRoute> {} });
    assert!(html.contains("prevent the name from being re-added"));
    assert!(html.contains("author-delete-duplicate-picker"));
    assert!(html.contains(">Delete<"));
}

#[test]
fn delete_modal_switches_to_merge_when_a_duplicate_is_picked() {
    let html = render_in_vdom(|| rsx! { dioxus_router::Router::<PickedRoute> {} });
    assert!(html.contains("Merge into"));
    assert!(html.contains("library scans will resolve"));
    assert!(html.contains("author-delete-duplicate-clear"));
    assert!(!html.contains("prevent the name from being re-added"));
}
