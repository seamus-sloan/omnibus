//! Series index page. Lists every series in the library as a browsable
//! surface, mirroring the `SeriesIndex` design comp from
//! `screens/indices.jsx`.

use std::cmp::Reverse;

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{IndexSort, SeriesSummary};

use super::index_shell::{index_page_early_return, use_index_page_shell, IndexPageState};
use crate::{data, index_prefs, use_server_url, Route};

/// Series index page — browse all series in the library.
#[component]
pub fn SeriesIndexPage() -> Element {
    let server_url = use_server_url();
    let shell = use_index_page_shell(
        move || {
            let url = server_url.clone();
            async move { data::list_series(&url).await }
        },
        || index_prefs::load().series_sort,
    );
    let IndexPageState {
        items: series,
        loading,
        error,
        mut filter,
        mut sort,
    } = shell;

    if let Some(early) = index_page_early_return(loading, error) {
        return early;
    }

    // Borrow the signal's contents instead of cloning — see authors_index
    // for the same shape and rationale (up to `INDEX_LIMIT` rows, copied
    // on every keystroke would be wasted work).
    let all = series.read();
    let total_series = all.len();
    let library_empty = all.is_empty();
    drop(all);

    // Memoized separately from the filter/sort pipeline below: this total is
    // unaffected by the filter text or sort key, so tying it to that memo
    // would recompute it on every keystroke for no reason.
    let total_books = use_memo(move || series.read().iter().map(|s| s.book_count).sum::<usize>());

    let filter_text = filter();
    let current_sort = sort();
    // Memoized so filter/sort only reruns when series/filter/sort change, not on every render.
    let filtered = use_memo(move || {
        let all = series.read();
        let q = filter();
        let s = sort();
        apply_filter_and_sort(&all, &q, s)
            .into_iter()
            .cloned()
            .collect::<Vec<SeriesSummary>>()
    });
    let filtered = filtered.read();

    rsx! {
        div { class: "idx-page",
            SeriesIndexHeader {
                view: SeriesHeaderView {
                    total_series,
                    total_books: total_books(),
                    filter: filter_text,
                    sort: current_sort,
                },
                on_filter: move |v| filter.set(v),
                on_sort: move |s: IndexSort| {
                    sort.set(s);
                    let mut prefs = index_prefs::load();
                    prefs.series_sort = s;
                    index_prefs::save(&prefs);
                },
            }
            {render_series_body(&filtered, library_empty)}
        }
    }
}

/// Filter by `query` (name + primary author, case-insensitive), then sort by `sort`.
fn apply_filter_and_sort<'a>(
    items: &'a [SeriesSummary],
    query: &str,
    sort: IndexSort,
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
    // `sort_by_cached_key` evaluates the key once per element instead of
    // re-running `to_lowercase()` on every comparison.
    match sort {
        IndexSort::Name => filtered.sort_by_cached_key(|a| sort_key(a).to_lowercase()),
        IndexSort::BookCount => {
            filtered.sort_by_cached_key(|a| (Reverse(a.book_count), sort_key(a).to_lowercase()))
        }
    }
    filtered
}

/// Body grid: empty-state copy when `filtered` is empty, otherwise the card grid.
fn render_series_body(filtered: &[SeriesSummary], library_empty: bool) -> Element {
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

/// Display state for [`SeriesIndexHeader`]: the two counts plus the current
/// filter text and sort mode. Grouped to keep the header under the prop cap.
#[derive(Clone, PartialEq)]
struct SeriesHeaderView {
    total_series: usize,
    total_books: usize,
    filter: String,
    sort: IndexSort,
}

/// Header: hero heading + subtitle, filter input, sort toggles.
#[component]
fn SeriesIndexHeader(
    view: SeriesHeaderView,
    on_filter: EventHandler<String>,
    on_sort: EventHandler<IndexSort>,
) -> Element {
    let SeriesHeaderView {
        total_series,
        total_books,
        filter,
        sort,
    } = view;
    rsx! {
        div { class: "idx-header",
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
                        "aria-pressed": if sort == IndexSort::Name { "true" } else { "false" },
                        "data-testid": "series-sort-name",
                        onclick: move |_| on_sort.call(IndexSort::Name),
                        "A\u{2013}Z"
                    }
                    button {
                        class: "idx-btn",
                        "aria-pressed": if sort == IndexSort::BookCount { "true" } else { "false" },
                        "data-testid": "series-sort-count",
                        onclick: move |_| on_sort.call(IndexSort::BookCount),
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
mod tests;
