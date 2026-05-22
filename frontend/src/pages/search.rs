//! Search results page — full-page version of the F1.5 palette.
//!
//! Reached from the palette by pressing Enter on the input (without
//! arrow-key navigation), or by clicking a Tag result. Reuses the
//! `data::search_palette` RPC and the same `PaletteResults` shape; each
//! row is a single [`Link`] to the relevant detail page so there's no
//! interactive selection state to coordinate with the palette overlay.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::PaletteResults;

use crate::{data, use_server_url, Route};

#[component]
pub fn SearchPage(query: String) -> Element {
    let server_url = use_server_url();
    let mut results: Signal<Option<PaletteResults>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    let q = query.clone();
    use_effect(move || {
        let url = url.clone();
        let q = q.clone();
        spawn(async move {
            loading.set(true);
            match data::search_palette(&url, &q).await {
                Ok(r) => {
                    results.set(Some(r));
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let header = rsx! {
        div { class: "search-page-header",
            Link {
                to: Route::Landing {},
                class: "btn ghost",
                "data-testid": "search-back",
                "\u{2190} Back to library"
            }
            h1 { class: "search-page-title",
                "Results for "
                span { class: "search-page-query", "\u{201c}{query}\u{201d}" }
            }
        }
    };

    if loading() {
        return rsx! {
            section { class: "search-page",
                {header}
                p { class: "subtitle", "Searching\u{2026}" }
            }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            section { class: "search-page",
                {header}
                p { role: "alert", class: "subtitle", "{msg}" }
            }
        };
    }
    let Some(r) = results() else {
        return rsx! {
            section { class: "search-page",
                {header}
                p { class: "subtitle", "No results for \u{201c}{query}\u{201d}." }
            }
        };
    };

    let total = r.total_count();
    rsx! {
        section { class: "search-page",
            {header}
            p { class: "subtitle", "data-testid": "search-result-count",
                "{total} result{plural(total)} \u{00b7} {r.duration_ms}ms"
            }
            if total == 0 {
                p { class: "subtitle", "data-testid": "search-empty",
                    "No results for \u{201c}{query}\u{201d}."
                }
            }
            if !r.books.is_empty() {
                SearchGroup { label: "Books", count: r.books.len() }
                ul { class: "search-results",
                    for book in r.books.iter().cloned() {
                        li {
                            Link {
                                to: Route::BookDetail { id: book.id },
                                class: "search-result-row",
                                "data-testid": "search-book-row",
                                div { class: "search-row-title", "{book.title}" }
                                div { class: "search-row-sub", "{book.author_display}" }
                            }
                        }
                    }
                }
            }
            if !r.authors.is_empty() {
                SearchGroup { label: "Authors", count: r.authors.len() }
                ul { class: "search-results",
                    for author in r.authors.iter().cloned() {
                        li {
                            Link {
                                to: Route::AuthorDetail { id: author.id },
                                class: "search-result-row",
                                "data-testid": "search-author-row",
                                div { class: "search-row-title", "{author.name}" }
                                div { class: "search-row-sub",
                                    "{author.book_count} book{plural(author.book_count as usize)}"
                                }
                            }
                        }
                    }
                }
            }
            if !r.series.is_empty() {
                SearchGroup { label: "Series", count: r.series.len() }
                ul { class: "search-results",
                    for s in r.series.iter().cloned() {
                        li {
                            Link {
                                to: Route::SeriesDetail { id: s.id },
                                class: "search-result-row",
                                "data-testid": "search-series-row",
                                div { class: "search-row-title", "{s.name}" }
                                div { class: "search-row-sub",
                                    "{s.book_count} book{plural(s.book_count as usize)}"
                                    if let Some(ref a) = s.author_display {
                                        " \u{00b7} {a}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !r.tags.is_empty() {
                SearchGroup { label: "Tags", count: r.tags.len() }
                ul { class: "search-results",
                    for tag in r.tags.iter().cloned() {
                        li {
                            Link {
                                to: Route::Search { query: format!("tag:{}", tag.name) },
                                class: "search-result-row",
                                "data-testid": "search-tag-row",
                                div { class: "search-row-title", "# {tag.name}" }
                                div { class: "search-row-sub",
                                    "{tag.book_count} book{plural(tag.book_count as usize)}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SearchGroup(label: &'static str, count: usize) -> Element {
    rsx! {
        h2 { class: "search-group-head label",
            "{label} \u{00b7} {count}"
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::{PaletteAuthorHit, PaletteBookHit, PaletteResults};

    #[test]
    fn plural_matches_count() {
        assert_eq!(plural(0), "s");
        assert_eq!(plural(1), "");
        assert_eq!(plural(2), "s");
    }

    #[test]
    fn palette_results_total_count_sums_groups() {
        let r = PaletteResults {
            query: "x".into(),
            books: vec![PaletteBookHit::default()],
            authors: vec![PaletteAuthorHit::default(), PaletteAuthorHit::default()],
            series: vec![],
            tags: vec![],
            duration_ms: 0,
        };
        assert_eq!(r.total_count(), 3);
    }
}
