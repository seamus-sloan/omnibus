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
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| Sort::Name);
    let (series, loading, error) = use_series_data(server_url);

    if loading() {
        return render_loading_state();
    }
    if let Some(msg) = error() {
        return render_error_state(&msg);
    }

    // Borrow the signal's contents instead of cloning — see authors_index
    // for the same shape and rationale (up to `INDEX_LIMIT` rows, copied
    // on every keystroke would be wasted work).
    let all = series.read();
    let total_series = all.len();
    let total_books: usize = all.iter().map(|s| s.book_count).sum();

    let filter_text = filter();
    let current_sort = sort();
    let filtered = apply_filter_and_sort(&all, &filter_text, current_sort);

    rsx! {
        div { class: "idx-page",
            SeriesIndexHeader {
                total_series,
                total_books,
                filter: filter_text,
                sort: current_sort,
                on_filter: move |v| filter.set(v),
                on_sort: move |s| sort.set(s),
            }
            {render_series_body(&filtered, all.is_empty())}
        }
    }
}

/// Spawn the one-shot `list_series` fetch and surface its outcome as three
/// signals (data, loading flag, error message). Declaring the signals and the
/// effect here keeps the hook order in `SeriesIndexPage` short and stable —
/// every render still calls `use_signal` three times and `use_effect` once,
/// in the same order, on every target.
fn use_series_data(
    server_url: String,
) -> (
    Signal<Vec<SeriesSummary>>,
    Signal<bool>,
    Signal<Option<String>>,
) {
    let mut series: Signal<Vec<SeriesSummary>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let url = server_url.clone();
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

    (series, loading, error)
}

/// Filter `items` by `query` (case-insensitive, matched against series name
/// and primary author), then sort the remaining references by the chosen
/// axis. Returns borrowed references so the caller can keep the original
/// `Vec` alive without per-keystroke cloning.
fn apply_filter_and_sort<'a>(
    items: &'a [SeriesSummary],
    query: &str,
    sort: Sort,
) -> Vec<&'a SeriesSummary> {
    let q = query.to_lowercase();
    let mut filtered: Vec<&SeriesSummary> = if q.is_empty() {
        items.iter().collect()
    } else {
        items
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&q)
                    || s.primary_author
                        .as_deref()
                        .map(|a| a.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .collect()
    };
    match sort {
        Sort::Name => filtered.sort_by_key(|a| sort_key(a).to_lowercase()),
        Sort::BookCount => filtered.sort_by(|a, b| {
            b.book_count
                .cmp(&a.book_count)
                .then_with(|| sort_key(a).to_lowercase().cmp(&sort_key(b).to_lowercase()))
        }),
    }
    filtered
}

/// Loading placeholder shown while `list_series` is in flight.
fn render_loading_state() -> Element {
    rsx! {
        p { class: "subtitle", "Loading\u{2026}" }
    }
}

/// Error state with the fetch message and a link back to the library.
fn render_error_state(msg: &str) -> Element {
    rsx! {
        p { role: "alert", class: "subtitle", "{msg}" }
        Link { to: Route::Landing {}, class: "btn", "Back to library" }
    }
}

/// Body grid: either the empty-state copy or the filtered card grid.
/// `library_empty` distinguishes "no series indexed yet" from "filter
/// matches nothing" so the copy can address each case directly.
fn render_series_body(filtered: &[&SeriesSummary], library_empty: bool) -> Element {
    rsx! {
        div { class: "idx-body",
            if filtered.is_empty() {
                p { class: "subtitle idx-empty",
                    if library_empty {
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

/// Header: breadcrumb, hero heading + subtitle, filter input, sort toggles.
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

    fn summary_full(name: &str, author: Option<&str>, book_count: usize) -> SeriesSummary {
        SeriesSummary {
            name: name.to_string(),
            primary_author: author.map(String::from),
            book_count,
            ..Default::default()
        }
    }

    #[test]
    fn apply_filter_and_sort_returns_all_when_query_is_empty() {
        let items = vec![
            summary_full("Foundation", Some("Asimov"), 7),
            summary_full("Dune", Some("Herbert"), 6),
        ];
        let out = apply_filter_and_sort(&items, "", Sort::Name);
        assert_eq!(out.len(), 2);
        // Sort::Name orders alphabetically by sort_key.
        assert_eq!(out[0].name, "Dune");
        assert_eq!(out[1].name, "Foundation");
    }

    #[test]
    fn apply_filter_and_sort_filters_by_name_case_insensitive() {
        let items = vec![
            summary_full("Foundation", Some("Asimov"), 7),
            summary_full("Dune", Some("Herbert"), 6),
        ];
        let out = apply_filter_and_sort(&items, "FOUND", Sort::Name);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Foundation");
    }

    #[test]
    fn apply_filter_and_sort_filters_by_primary_author() {
        let items = vec![
            summary_full("Foundation", Some("Asimov"), 7),
            summary_full("Dune", Some("Herbert"), 6),
        ];
        let out = apply_filter_and_sort(&items, "herbert", Sort::Name);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Dune");
    }

    #[test]
    fn apply_filter_and_sort_sorts_by_book_count_descending_then_name() {
        let items = vec![
            summary_full("Alpha", None, 3),
            summary_full("Bravo", None, 7),
            summary_full("Charlie", None, 7),
        ];
        let out = apply_filter_and_sort(&items, "", Sort::BookCount);
        assert_eq!(out[0].name, "Bravo");
        assert_eq!(out[1].name, "Charlie");
        assert_eq!(out[2].name, "Alpha");
    }
}
