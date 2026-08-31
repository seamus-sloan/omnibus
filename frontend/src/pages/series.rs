//! Series discovery page — shows series details and ordered books, mirroring
//! the `SeriesScreen` design comp from `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, SeriesDetail};

use crate::components::atrium::Cover;
use crate::components::{disc_back_link, PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

/// Renders the series discovery page.
#[component]
pub fn SeriesPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut series: Signal<Option<SeriesDetail>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    crate::use_page_title(move || series.read().as_ref().map(|s| s.name.clone()));

    // See `BookDetailPage` for why `id` needs `use_reactive!`.
    let url = server_url.clone();
    let generation = crate::use_cache_generation();
    use_effect(use_reactive!(|id| {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let url = url.clone();
        spawn(async move {
            let same_series = series.peek().as_ref().map(|s| s.id) == Some(id);
            if !same_series {
                loading.set(true);
            }
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

    rsx! {
        div { class: "disc-page", style: "--accent: {accent}",
            {series_header(&s)}
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

/// Series page header: mobile back affordance and the italic-first-word
/// hero title.
fn series_header(s: &SeriesDetail) -> Element {
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
            {disc_back_link(Route::SeriesIndex {}, "Back to series", "series-back")}
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

/// The series card's eyebrow — `"Book #1 · May 2016"`, dropping either half
/// when it's absent. Only a real date carries the `·` separator: a sentinel or
/// truly-absent date drops the whole date slot rather than trailing a bare
/// `· —`, so two equally-dateless books read the same (#2294, #2360).
fn series_card_eyebrow(book: &EbookMetadata) -> String {
    let idx = book
        .series_index
        .as_deref()
        .map(|idx| format!("Book #{idx}"));
    let date = book
        .published
        .as_deref()
        .and_then(crate::format::format_date_month_year_opt);
    match (idx, date) {
        (Some(idx), Some(date)) => format!("{idx} \u{b7} {date}"),
        (Some(part), None) | (None, Some(part)) => part,
        (None, None) => String::new(),
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
                span { class: "label", "{series_card_eyebrow(book)}" }
                h3 { class: "series-card-title",
                    Link {
                        to: Route::BookDetail { uuid: book.unique_identifier.clone().unwrap_or_default() },
                        class: "series-card-hit",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn book(series_index: Option<&str>, published: Option<&str>) -> EbookMetadata {
        EbookMetadata {
            series_index: series_index.map(str::to_string),
            published: published.map(str::to_string),
            ..EbookMetadata::default()
        }
    }

    #[test]
    fn eyebrow_pairs_the_index_with_a_real_date() {
        assert_eq!(
            series_card_eyebrow(&book(Some("1"), Some("2016-05-02"))),
            "Book #1 \u{b7} May 2016"
        );
    }

    #[test]
    fn eyebrow_reads_the_same_for_an_absent_and_a_sentinel_date() {
        // Both dateless books must render "Book #1" with no trailing `· —`
        // (#2294, #2360) — the whole point is that they agree.
        let absent = series_card_eyebrow(&book(Some("1"), None));
        let sentinel = series_card_eyebrow(&book(Some("1"), Some("0101-01-01T00:00:00+00:00")));
        assert_eq!(absent, "Book #1");
        assert_eq!(sentinel, "Book #1");
        assert!(!sentinel.contains('\u{2014}'));
    }

    #[test]
    fn eyebrow_drops_the_leading_separator_when_only_a_date_is_present() {
        assert_eq!(
            series_card_eyebrow(&book(None, Some("2016-05"))),
            "May 2016"
        );
        assert_eq!(series_card_eyebrow(&book(None, None)), "");
    }
}
