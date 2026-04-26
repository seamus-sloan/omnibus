use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata};

use crate::{data, use_search_query, use_server_url, Route};

/// Landing page — loads the configured ebook library and renders every book
/// in a single table with cover thumbnails and the common metadata columns.
/// Clicking (or pressing Enter / Space on) a row navigates to the stub
/// `/books/:id` detail page. The detail page itself is still a TODO.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    let mut library = use_signal(EbookLibrary::default);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    // Search box lives in the top nav; the query is shared via context so
    // typing on any route drives the landing results without a route param.
    let query = use_search_query().0;

    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let url = url_for_fetch.clone();
        let q = query();
        spawn(async move {
            loading.set(true);
            let trimmed = q.trim();
            let result = if trimmed.is_empty() {
                data::get_ebooks(&url).await
            } else {
                data::search_ebooks(&url, trimmed).await
            };
            match result {
                Ok(lib) => {
                    library.set(lib);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let server_url_for_row = server_url.clone();

    let lib = library();
    let is_loading = loading();
    let page_error = error();
    let book_count = lib.books.len();

    rsx! {
        section { class: "card",
            h1 { "Your Library" }
            p { class: "subtitle",
                if let Some(path) = lib.path.as_ref() {
                    "{path} · {book_count} book(s)"
                } else {
                    "Configure your ebook library path in Settings."
                }
            }
            if let Some(msg) = page_error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
            if let Some(msg) = lib.error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
        }

        div {
            id: "ebook-table",
            "data-testid": "ebook-table",
            class: "ebook-table-wrap",
            if is_loading {
                p { class: "library-empty", "Loading..." }
            } else if lib.books.is_empty() && lib.error.is_none() && page_error.is_none() {
                p { class: "library-empty", "No ebooks found." }
            } else {
                table { class: "ebook-table",
                    thead {
                        tr {
                            th { class: "ebook-col-cover", "Cover" }
                            th { class: "ebook-col-title", "Title" }
                            th { class: "ebook-col-author", "Author" }
                            th { class: "ebook-col-series", "Series" }
                            th { class: "ebook-col-publisher", "Publisher" }
                            th { class: "ebook-col-published", "Published" }
                            th { class: "ebook-col-language", "Language" }
                        }
                    }
                    tbody {
                        for book in lib.books.into_iter() {
                            EbookRow {
                                key: "{book.filename}",
                                book: book,
                                server_url: server_url_for_row.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EbookRow(book: EbookMetadata, server_url: String) -> Element {
    let id = book.id;
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    // Stable per-row testid for Playwright. Derived from the on-disk filename
    // (stem only, lowercased, non-alphanumerics collapsed to `-`) so fixtures
    // can look a row up by the same slug they ship under.
    let row_testid = format!("ebook-row-{}", row_slug(&book.filename));
    // Combine the relative cover URL the server returned with the client's
    // base URL. Web sees an empty base (same-origin); mobile prepends its
    // configured `ServerUrl`.
    let cover_src = book
        .cover_url
        .as_deref()
        .map(|rel| format!("{server_url}{rel}"));
    let series_line = match (book.series.as_deref(), book.series_index.as_deref()) {
        (Some(s), Some(i)) => format!("{s} #{i}"),
        (Some(s), None) => s.to_string(),
        _ => String::new(),
    };
    let authors = contributor_names(&book.creators);

    // `use_navigator` returns a `Copy` handle, so each handler can call it
    // independently without cloning.
    let nav = use_navigator();

    rsx! {
        tr {
            class: "ebook-row",
            "data-testid": "{row_testid}",
            id: "{row_testid}",
            role: "button",
            tabindex: "0",
            aria_label: "Open details for {display_title}",
            onclick: move |_| {
                nav.push(Route::BookDetail { id });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                // Activate the row on Enter or Space, matching <button> semantics.
                let key = evt.key();
                if key == Key::Enter || key == Key::Character(" ".to_string()) {
                    evt.prevent_default();
                    nav.push(Route::BookDetail { id });
                }
            },
            td { class: "ebook-col-cover", "data-testid": "ebook-cell-cover",
                if let Some(src) = cover_src.as_ref() {
                    img { class: "ebook-thumb", src: "{src}", alt: "Cover of {display_title}", loading: "lazy" }
                } else {
                    div { class: "ebook-thumb ebook-thumb-fallback", "—" }
                }
            }
            td { class: "ebook-col-title", "data-testid": "ebook-cell-title",
                div { class: "ebook-title-cell", "{display_title}" }
                if let Some(err) = book.error.as_ref() {
                    div { class: "error", "⚠ {err}" }
                }
            }
            td { class: "ebook-col-author", "data-testid": "ebook-cell-author", "{authors}" }
            td { class: "ebook-col-series", "data-testid": "ebook-cell-series", "{series_line}" }
            td { class: "ebook-col-publisher", "data-testid": "ebook-cell-publisher", {book.publisher.as_deref().unwrap_or("")} }
            td { class: "ebook-col-published", "data-testid": "ebook-cell-published", {book.published.as_deref().unwrap_or("")} }
            td { class: "ebook-col-language", "data-testid": "ebook-cell-language", {book.language.as_deref().unwrap_or("")} }
        }
    }
}

fn contributor_names(list: &[Contributor]) -> String {
    let mut out = String::new();
    for (i, c) in list.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&c.name);
    }
    out
}

/// Stable Playwright row id derived from the ebook's on-disk filename:
/// strip directories and extension, lowercase, then collapse runs of
/// non-alphanumeric ASCII characters into a single `-` (with leading and
/// trailing dashes trimmed). The Playwright fixture table mirrors this
/// derivation so each `FIXTURE_BOOKS[i].slug` matches the row's testid.
fn row_slug(filename: &str) -> String {
    // Take the basename so nested paths (e.g. "series/vol1/deep.epub") still
    // produce a clean slug from just the file's stem.
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let stem = basename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(basename);
    let lower = stem.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_dash = true; // suppress leading dashes
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::row_slug;

    #[test]
    fn row_slug_lowercases_and_strips_extension() {
        assert_eq!(row_slug("Alpha.epub"), "alpha");
    }

    #[test]
    fn row_slug_collapses_runs_of_non_alphanumerics() {
        assert_eq!(row_slug("Beta in the Series.epub"), "beta-in-the-series");
    }

    #[test]
    fn row_slug_uses_basename_for_nested_paths() {
        assert_eq!(row_slug("series/vol1/Deep Book.epub"), "deep-book");
    }

    #[test]
    fn row_slug_trims_trailing_dashes() {
        assert_eq!(row_slug("weird---name!!!.epub"), "weird-name");
    }

    #[test]
    fn row_slug_handles_filename_without_extension() {
        assert_eq!(row_slug("plain"), "plain");
    }
}
