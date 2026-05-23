//! Authors index page (F1.12). Lists every author in the library as a
//! browsable surface, mirroring the `AuthorsIndex` design comp from
//! `screens/indices.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::AuthorSummary;

use crate::{data, use_server_url, Route};

/// Sort axes exposed in the toolbar. Mirrors the design's "Last name A–Z /
/// Most books" toggle. "Recently added" and "Highest rated" from the comp
/// require fields the v1 summary doesn't carry — deferred to a follow-up
/// once `AuthorSummary` grows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Sort {
    Name,
    BookCount,
}

#[component]
pub fn AuthorsIndexPage() -> Element {
    let server_url = use_server_url();
    let mut authors: Signal<Vec<AuthorSummary>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| Sort::Name);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::list_authors(&url).await {
                Ok(a) => {
                    authors.set(a);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    if loading() {
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }

    let all = authors();
    let total_authors = all.len();
    let total_books: usize = all.iter().map(|a| a.book_count).sum();

    // Client-side filter + sort.
    let q = filter().to_lowercase();
    let mut filtered: Vec<&AuthorSummary> = if q.is_empty() {
        all.iter().collect()
    } else {
        all.iter()
            .filter(|a| a.name.to_lowercase().contains(&q))
            .collect()
    };
    match sort() {
        Sort::Name => filtered.sort_by_key(|a| sort_key(a).to_lowercase()),
        Sort::BookCount => filtered.sort_by(|a, b| {
            b.book_count
                .cmp(&a.book_count)
                .then_with(|| sort_key(a).to_lowercase().cmp(&sort_key(b).to_lowercase()))
        }),
    }

    // Group by first letter of sort key (last name when available, else name).
    // Only meaningful when sorting by name.
    let show_letters = matches!(sort(), Sort::Name);
    let mut letters: Vec<(char, Vec<&AuthorSummary>)> = Vec::new();
    if show_letters {
        for a in &filtered {
            let l = first_letter(a);
            match letters.last_mut() {
                Some((existing, group)) if *existing == l => group.push(a),
                _ => letters.push((l, vec![a])),
            }
        }
    }
    let alphabet: Vec<char> = ('A'..='Z').collect();
    let present_letters: std::collections::HashSet<char> =
        letters.iter().map(|(l, _)| *l).collect();

    rsx! {
        div { class: "idx-page",
            // Header
            div { class: "idx-header",
                nav {
                    class: "breadcrumb",
                    aria_label: "breadcrumb",
                    Link { to: Route::Landing {}, "Library" }
                    span { class: "breadcrumb-sep", " › " }
                    span { "Authors" }
                }
                div { class: "idx-head-row",
                    div {
                        span { class: "label", "Library lens" }
                        h1 { class: "disc-hero-title",
                            "By "
                            em { "author" }
                            "."
                        }
                        p { class: "idx-subtitle",
                            "{total_authors} authors across {total_books} books \u{b7} the people in your shelves."
                        }
                    }
                }

                // Toolbar
                div { class: "idx-toolbar",
                    div { class: "idx-search",
                        input {
                            r#type: "search",
                            placeholder: "Filter authors by name\u{2026}",
                            aria_label: "Filter authors by name",
                            value: "{filter}",
                            "data-testid": "authors-filter",
                            oninput: move |e| filter.set(e.value()),
                        }
                    }
                    div { class: "idx-sort",
                        span { class: "label", "Sort" }
                        button {
                            class: "idx-btn",
                            "aria-pressed": if sort() == Sort::Name { "true" } else { "false" },
                            "data-testid": "authors-sort-name",
                            onclick: move |_| sort.set(Sort::Name),
                            "Last name A\u{2013}Z"
                        }
                        button {
                            class: "idx-btn",
                            "aria-pressed": if sort() == Sort::BookCount { "true" } else { "false" },
                            "data-testid": "authors-sort-count",
                            onclick: move |_| sort.set(Sort::BookCount),
                            "Most books"
                        }
                    }
                }

                // Letter jump strip
                if show_letters {
                    div { class: "idx-letters",
                        for l in alphabet.iter() {
                            {
                                let has = present_letters.contains(l);
                                let class = if has { "idx-letter idx-letter-on" } else { "idx-letter" };
                                rsx! {
                                    span { class: "{class}", "{l}" }
                                }
                            }
                        }
                        span { class: "idx-letters-spacer" }
                        span { class: "mono idx-letters-count",
                            "{letters.len()} letters \u{b7} {filtered.len()} authors"
                        }
                    }
                }
            }

            // Body
            div { class: "idx-body",
                if filtered.is_empty() {
                    p { class: "subtitle idx-empty",
                        if all.is_empty() {
                            "No authors yet \u{2014} the library is empty."
                        } else {
                            "No authors match that filter."
                        }
                    }
                } else if show_letters {
                    for (letter, group) in letters.iter() {
                        section { class: "idx-letter-section",
                            div { class: "idx-letter-rail",
                                div { class: "idx-letter-rail-glyph", "{letter}" }
                                div { class: "idx-letter-rail-count mono", "{group.len()}" }
                            }
                            div { class: "idx-card-grid",
                                for a in group.iter() {
                                    {render_author_card(a)}
                                }
                            }
                        }
                    }
                } else {
                    div { class: "idx-card-grid idx-card-grid--flat",
                        for a in filtered.iter() {
                            {render_author_card(a)}
                        }
                    }
                }
            }
        }
    }
}

fn render_author_card(a: &AuthorSummary) -> Element {
    let accent = a.accent.clone().unwrap_or_else(|| "var(--accent)".into());
    let name = a.name.clone();
    let id = a.id;
    let count = a.book_count;
    let parts: Vec<&str> = name.rsplitn(2, ' ').collect();
    // `rsplitn(2)` reverses; last name is parts[0], rest is parts[1] when present.
    let (rest, last) = if parts.len() == 2 {
        (parts[1].to_string(), parts[0].to_string())
    } else {
        (String::new(), name.clone())
    };
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .next()
        .unwrap_or('?');

    rsx! {
        Link {
            to: Route::AuthorDetail { id },
            class: "idx-card idx-card-author",
            style: "--accent: {accent}",
            "data-testid": "author-card",
            div { class: "idx-card-avatar", "{initial}" }
            div { class: "idx-card-body",
                div { class: "idx-card-title",
                    if !rest.is_empty() {
                        span { class: "idx-card-name-rest", "{rest} " }
                    }
                    span { class: "idx-card-name-last", "{last}" }
                }
                div { class: "idx-card-stats",
                    div { class: "idx-stat",
                        div { class: "mono idx-stat-label", "Books" }
                        div { class: "idx-stat-value", "{count}" }
                    }
                }
            }
        }
    }
}

fn first_letter(a: &AuthorSummary) -> char {
    sort_key(a)
        .chars()
        .next()
        .unwrap_or('#')
        .to_uppercase()
        .next()
        .unwrap_or('#')
}

/// Sort key for the author: prefers the persisted `sort` value (already
/// last-name-first when available) and falls back to a derived "Last,
/// First" form so display names like "Ada Lovelace" still group under L.
fn sort_key(a: &AuthorSummary) -> String {
    if let Some(s) = a.sort.as_deref().filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    // Heuristic fallback: split on last space; treat the tail as the
    // surname. Names without a space (mononyms) fall through unchanged.
    let parts: Vec<&str> = a.name.rsplitn(2, ' ').collect();
    if parts.len() == 2 {
        format!("{}, {}", parts[0], parts[1])
    } else {
        a.name.clone()
    }
}
