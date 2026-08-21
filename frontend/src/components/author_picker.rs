//! Inline canonical-author picker: a filter input over the author list with
//! one button per match. Shared by the delete-as-duplicate flows (author
//! page modal, cleanup review Delete card) and the blocklist conversion
//! list. Web-only, like the admin cleanup surfaces that mount it.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::AuthorSummary;

use crate::{data, use_server_url};

/// The author a picker selection resolved to.
#[derive(Clone, PartialEq)]
pub struct AuthorPick {
    pub id: i64,
    pub name: String,
}

/// Matches rendered under the filter input at most — the input narrows, the
/// cap keeps a one-letter query from dumping the whole library.
const PICKER_MAX_MATCHES: usize = 8;

/// Filter input + match buttons. Fetches the author list once after mount
/// (SSR and first hydration render the empty state identically) and filters
/// client-side; `exclude_id` drops the entity being deleted from the
/// candidates so it can't be picked as its own canonical.
#[component]
pub fn AuthorPicker(
    #[props(default)] exclude_id: Option<i64>,
    testid: String,
    on_pick: EventHandler<AuthorPick>,
) -> Element {
    let server_url = use_server_url();
    let mut authors: Signal<Option<Vec<AuthorSummary>>> = use_signal(|| None);
    let mut query = use_signal(String::new);
    let mut load_error = use_signal(|| false);

    use_effect(move || {
        let server_url = server_url.clone();
        spawn(async move {
            match data::list_authors(&server_url).await {
                Ok(list) => authors.set(Some(list)),
                Err(_) => load_error.set(true),
            }
        });
    });

    let matches = author_matches(&authors.read(), &query.read(), exclude_id);
    rsx! {
        div { class: "author-picker", "data-testid": "{testid}",
            input {
                class: "author-picker-input",
                r#type: "text",
                placeholder: "Find an author\u{2026}",
                aria_label: "Find an author",
                "data-testid": "{testid}-input",
                value: "{query}",
                oninput: move |evt| query.set(evt.value()),
            }
            if load_error() {
                p { class: "settings-status error", role: "status", "Failed to load authors." }
            }
            if !matches.is_empty() {
                ul { class: "author-picker-matches", "data-testid": "{testid}-matches",
                    for m in matches {
                        li { key: "{m.id}",
                            button {
                                class: "btn author-picker-match",
                                "data-testid": "{testid}-match-{m.id}",
                                onclick: {
                                    let m = m.clone();
                                    move |_| on_pick.call(m.clone())
                                },
                                "{m.name}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The candidates a query resolves to: case-insensitive substring match on
/// the name, minus `exclude_id`, capped at [`PICKER_MAX_MATCHES`]. An empty
/// query matches nothing — the list only appears once the admin types.
fn author_matches(
    authors: &Option<Vec<AuthorSummary>>,
    query: &str,
    exclude_id: Option<i64>,
) -> Vec<AuthorPick> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let Some(authors) = authors else {
        return Vec::new();
    };
    authors
        .iter()
        .filter(|a| Some(a.id) != exclude_id)
        .filter(|a| a.name.to_lowercase().contains(&needle))
        .take(PICKER_MAX_MATCHES)
        .map(|a| AuthorPick {
            id: a.id,
            name: a.name.clone(),
        })
        .collect()
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use omnibus_shared::AuthorSummary;

    use super::{author_matches, AuthorPicker};
    use crate::test_support::render_in_vdom;
    use dioxus::prelude::*;

    fn summary(id: i64, name: &str) -> AuthorSummary {
        AuthorSummary {
            id,
            name: name.into(),
            sort: None,
            book_count: 1,
            accent: None,
            has_photo: false,
        }
    }

    #[test]
    fn author_matches_filters_case_insensitively_and_excludes_the_source() {
        let authors = Some(vec![
            summary(1, "Andy Weir"),
            summary(2, "Weir, Andy"),
            summary(3, "Ursula K. Le Guin"),
        ]);
        let picks = author_matches(&authors, "weir", Some(2));
        let names: Vec<&str> = picks.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Andy Weir"]);
    }

    #[test]
    fn author_matches_returns_nothing_for_an_empty_query() {
        let authors = Some(vec![summary(1, "Andy Weir")]);
        assert!(author_matches(&authors, "  ", None).is_empty());
    }

    #[test]
    fn author_picker_renders_the_filter_input() {
        let html = render_in_vdom(|| {
            rsx! {
                AuthorPicker {
                    testid: "pick-test".to_string(),
                    on_pick: EventHandler::new(|_| {}),
                }
            }
        });
        assert!(html.contains("pick-test-input"));
        assert!(html.contains("Find an author"));
    }
}
