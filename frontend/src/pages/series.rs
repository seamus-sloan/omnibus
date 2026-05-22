//! Series discovery page — shows series details and ordered books, mirroring
//! the `SeriesScreen` design comp from `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::SeriesDetail;

use crate::components::atrium::Cover;
use crate::{data, use_server_url, Route};

#[component]
pub fn SeriesPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut series: Signal<Option<SeriesDetail>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_series(&url, id).await {
                Ok(s) => {
                    series.set(s);
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
    let Some(s) = series() else {
        return rsx! {
            p { class: "subtitle", "Series not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    };

    render_series(s)
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn render_series(s: SeriesDetail) -> Element {
    // Derive accent from the first book that has one.
    let accent = s
        .books
        .iter()
        .find_map(|b| b.accent.as_deref())
        .unwrap_or("var(--accent)");

    // Primary author from the first book's first creator.
    let primary_author = s
        .books
        .iter()
        .find_map(|b| b.creators.first().map(|c| c.name.clone()))
        .unwrap_or_default();

    // Split series name for italic styling on key word.
    let title_parts: Vec<&str> = s.name.splitn(2, ' ').collect();
    let title_first = title_parts.first().copied().unwrap_or("");
    let title_rest = if title_parts.len() > 1 {
        title_parts[1]
    } else {
        ""
    };

    rsx! {
        div { class: "disc-page", style: "--accent: {accent}",
            // Header
            div { class: "disc-series-header",
                nav { class: "breadcrumb",
                    Link { to: Route::Landing {}, "Library" }
                    span { " › " }
                    if !primary_author.is_empty() {
                        span { "{primary_author}" }
                        span { " › " }
                    }
                    span { "{s.name}" }
                }
                div { class: "disc-series-head-row",
                    div {
                        span { class: "label", "Series · {s.book_count} in library" }
                        h1 { class: "disc-hero-title",
                            "The "
                            em { "{title_first}" }
                            if !title_rest.is_empty() {
                                " {title_rest}"
                            }
                        }
                    }
                }
            }

            // Body: card grid of books
            div { class: "disc-body",
                div { class: "series-cards",
                    for book in s.books.iter() {
                        article { class: "card series-card",
                            div { class: "series-card-cover",
                                Link {
                                    to: Route::BookDetail { id: book.id },
                                    Cover { book: book.clone() }
                                }
                            }
                            div { class: "series-card-info",
                                span { class: "label",
                                    if let Some(ref idx) = book.series_index {
                                        "Book #{idx}"
                                    }
                                    if let Some(ref year) = book.published {
                                        " · {year}"
                                    }
                                }
                                h3 { class: "series-card-title",
                                    Link {
                                        to: Route::BookDetail { id: book.id },
                                        "{book.title.as_deref().unwrap_or(&book.filename)}"
                                    }
                                }
                                if let Some(ref desc) = book.description {
                                    div { class: "series-card-desc",
                                        dangerous_inner_html: "{desc}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
