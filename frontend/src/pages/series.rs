//! Series discovery page — shows series details and ordered books, mirroring
//! the `SeriesScreen` design comp from `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, SeriesDetail};

use crate::components::atrium::Cover;
use crate::components::{PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

/// Renders the series discovery page.
#[component]
pub fn SeriesPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut series: Signal<Option<SeriesDetail>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // See `BookDetailPage` for why `id` needs `use_reactive!`.
    let url = server_url.clone();
    use_effect(use_reactive!(|id| {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_series(&url, id).await {
                Ok(s) => {
                    series.set(s);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    }));

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }
    let Some(s) = series() else {
        return rsx! { PageNotFound { subject: "Series", back_to: Route::Landing {} } };
    };

    render_series(s)
}

fn render_series(s: SeriesDetail) -> Element {
    // Derive accent from the first book that has one.
    let accent = s
        .books
        .iter()
        .find_map(|b| b.accent.as_deref())
        .unwrap_or("var(--accent)");

    // Primary author from the first book's first creator. Capture the
    // author id alongside the name so the breadcrumb can render a Link
    // when the row is resolvable; books whose first creator is an
    // override-only string (no `authors` row) fall back to a plain span.
    let (primary_author, primary_author_id) = s
        .books
        .iter()
        .find_map(|b| b.creators.first().map(|c| (c.name.clone(), c.id)))
        .unwrap_or_default();

    rsx! {
        div { class: "disc-page", style: "--accent: {accent}",
            {series_header(&s, &primary_author, primary_author_id)}
            div { class: "disc-body",
                div { class: "series-cards",
                    for book in s.books.iter() {
                        {series_book_row(book)}
                    }
                }
            }
        }
    }
}

/// Series page header: breadcrumb (with an optional linked author crumb) and
/// the italic-first-word hero title.
fn series_header(
    s: &SeriesDetail,
    primary_author: &str,
    primary_author_id: Option<i64>,
) -> Element {
    // Split series name for italic styling on key word.
    let title_parts: Vec<&str> = s.name.splitn(2, ' ').collect();
    let title_first = title_parts.first().copied().unwrap_or("");
    let title_rest = if title_parts.len() > 1 {
        title_parts[1]
    } else {
        ""
    };
    rsx! {
        div { class: "disc-series-header",
            nav { class: "breadcrumb", aria_label: "breadcrumb",
                Link { to: Route::Landing {}, "Library" }
                span { class: "breadcrumb-sep", " › " }
                if !primary_author.is_empty() {
                    if let Some(author_id) = primary_author_id {
                        Link {
                            to: Route::AuthorDetail { id: author_id },
                            "data-testid": "series-crumb-author",
                            "{primary_author}"
                        }
                    } else {
                        span { "data-testid": "series-crumb-author", "{primary_author}" }
                    }
                    span { class: "breadcrumb-sep", " › " }
                }
                span { "{s.name}" }
            }
            div { class: "disc-series-head-row",
                div {
                    span { class: "label", "Series · {s.book_count} in library" }
                    h1 { class: "disc-hero-title",
                        em { "{title_first}" }
                        if !title_rest.is_empty() {
                            " {title_rest}"
                        }
                    }
                }
            }
        }
    }
}

/// One book card in the series grid: cover, index/year label, title link, and
/// an optional description.
fn series_book_row(book: &EbookMetadata) -> Element {
    rsx! {
        article {
            key: "{book.id}",
            class: "card series-card",
            div { class: "series-card-cover",
                Link {
                    to: Route::BookDetail { uuid: book.unique_identifier.clone().unwrap_or_default() },
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
                        to: Route::BookDetail { uuid: book.unique_identifier.clone().unwrap_or_default() },
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
