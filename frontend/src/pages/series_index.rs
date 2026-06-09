//! Series index page. Lists every series in the library as a browsable
//! surface, mirroring the `SeriesIndex` design comp from
//! `screens/indices.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::SeriesSummary;

use crate::{data, use_server_url, Route};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Sort {
    Name,
    BookCount,
}

/// Series index page — browse all series in the library.
#[component]
pub fn SeriesIndexPage() -> Element {
    let server_url = use_server_url();
    let mut series: Signal<Vec<SeriesSummary>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| Sort::Name);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::list_series(&url).await {
                Ok(s) => {
                    series.set(s);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
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

    // Borrow the signal's contents instead of cloning — see authors_index
    // for the same shape and rationale (up to `INDEX_LIMIT` rows, copied
    // on every keystroke would be wasted work).
    let all = series.read();
    let total_series = all.len();
    let total_books: usize = all.iter().map(|s| s.book_count).sum();

    let q = filter.read().to_lowercase();
    let mut filtered: Vec<&SeriesSummary> = if q.is_empty() {
        all.iter().collect()
    } else {
        all.iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&q)
                    || s.primary_author
                        .as_deref()
                        .map(|a| a.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
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

    rsx! {
        div { class: "idx-page",
            SeriesIndexHeader {
                total_series,
                total_books,
                filter: filter(),
                sort: sort(),
                on_filter: move |v| filter.set(v),
                on_sort: move |s| sort.set(s),
            }
            div { class: "idx-body",
                if filtered.is_empty() {
                    p { class: "subtitle idx-empty",
                        if all.is_empty() {
                            "No series yet \u{2014} index a library to see them here."
                        } else {
                            "No series match that filter."
                        }
                    }
                } else {
                    div { class: "idx-series-grid",
                        for s in filtered.iter() {
                            div { key: "{s.id}", {render_series_card(s)} }
                        }
                    }
                }
            }
        }
    }
}

/// Header section: breadcrumb, hero heading + subtitle, filter input, and
/// sort toggle buttons.
#[component]
fn SeriesIndexHeader(
    total_series: usize,
    total_books: usize,
    filter: String,
    sort: Sort,
    on_filter: EventHandler<String>,
    on_sort: EventHandler<Sort>,
) -> Element {
    rsx! {
        div { class: "idx-header",
            nav {
                class: "breadcrumb",
                aria_label: "breadcrumb",
                Link { to: Route::Landing {}, "Library" }
                span { class: "breadcrumb-sep", " › " }
                span { "Series" }
            }
            div { class: "idx-head-row",
                div {
                    span { class: "label", "Library lens" }
                    h1 { class: "disc-hero-title",
                        "By "
                        em { "series" }
                        "."
                    }
                    p { class: "idx-subtitle",
                        "{total_series} series \u{b7} {total_books} books across them."
                    }
                }
            }
            div { class: "idx-toolbar",
                div { class: "idx-search",
                    input {
                        r#type: "search",
                        placeholder: "Filter series by name or author\u{2026}",
                        aria_label: "Filter series",
                        value: "{filter}",
                        "data-testid": "series-filter",
                        oninput: move |e| on_filter.call(e.value()),
                    }
                }
                div { class: "idx-sort",
                    span { class: "label", "Sort" }
                    button {
                        class: "idx-btn",
                        "aria-pressed": if sort == Sort::Name { "true" } else { "false" },
                        "data-testid": "series-sort-name",
                        onclick: move |_| on_sort.call(Sort::Name),
                        "A\u{2013}Z"
                    }
                    button {
                        class: "idx-btn",
                        "aria-pressed": if sort == Sort::BookCount { "true" } else { "false" },
                        "data-testid": "series-sort-count",
                        onclick: move |_| on_sort.call(Sort::BookCount),
                        "Most books"
                    }
                }
            }
        }
    }
}

fn render_series_card(s: &SeriesSummary) -> Element {
    let accent = s.accent.clone().unwrap_or_else(|| "var(--accent)".into());
    let name = s.name.clone();
    let id = s.id;
    let count = s.book_count;
    let author = s.primary_author.clone().unwrap_or_default();

    // Italicize the last word of the series name, matching the design.
    let parts: Vec<&str> = name.rsplitn(2, ' ').collect();
    let (rest, last) = if parts.len() == 2 {
        (parts[1].to_string(), parts[0].to_string())
    } else {
        (String::new(), name.clone())
    };

    rsx! {
        Link {
            to: Route::SeriesDetail { id },
            class: "idx-card idx-card-series",
            style: "--accent: {accent}",
            "data-testid": "series-card",
            div { class: "idx-series-spine", aria_hidden: "true" }
            div { class: "idx-card-body",
                div { class: "idx-card-title",
                    if !rest.is_empty() {
                        span { class: "idx-card-name-rest", "{rest} " }
                    }
                    span { class: "idx-card-name-last", "{last}" }
                }
                if !author.is_empty() {
                    div { class: "mono idx-series-author", "{author}" }
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

fn sort_key(s: &SeriesSummary) -> String {
    if let Some(v) = s.sort.as_deref().filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    // Strip a leading "The " so "The Foo Bar" sorts under F.
    let n = &s.name;
    let stripped = n.strip_prefix("The ").or_else(|| n.strip_prefix("the "));
    stripped.unwrap_or(n).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, sort: Option<&str>) -> SeriesSummary {
        SeriesSummary {
            name: name.to_string(),
            sort: sort.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn sort_key_prefers_explicit_sort_field() {
        let s = summary("The Expanse", Some("Expanse"));
        assert_eq!(sort_key(&s), "Expanse");
    }

    #[test]
    fn sort_key_ignores_empty_sort_field() {
        let s = summary("Foundation", Some(""));
        assert_eq!(sort_key(&s), "Foundation");
    }

    #[test]
    fn sort_key_strips_leading_the_in_both_cases() {
        assert_eq!(sort_key(&summary("The Foo Bar", None)), "Foo Bar");
        assert_eq!(sort_key(&summary("the foo bar", None)), "foo bar");
    }

    #[test]
    fn sort_key_leaves_non_the_prefix_intact() {
        // "Theology" must not have "The" stripped — only the "The " word.
        assert_eq!(sort_key(&summary("Theology", None)), "Theology");
        assert_eq!(sort_key(&summary("Dune", None)), "Dune");
    }
}
