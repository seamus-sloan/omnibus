use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata};

use crate::{data, use_server_url, Route};

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

    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let url = url_for_fetch.clone();
        spawn(async move {
            match data::get_ebooks(&url).await {
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
                        for (idx, book) in lib.books.into_iter().enumerate() {
                            EbookRow {
                                key: "{book.filename}",
                                id: idx,
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
fn EbookRow(id: usize, book: EbookMetadata, server_url: String) -> Element {
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
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
            "data-testid": "ebook-row",
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
            td { class: "ebook-col-cover",
                if let Some(src) = cover_src.as_ref() {
                    img { class: "ebook-thumb", src: "{src}", alt: "Cover of {display_title}", loading: "lazy" }
                } else {
                    div { class: "ebook-thumb ebook-thumb-fallback", "—" }
                }
            }
            td { class: "ebook-col-title",
                div { class: "ebook-title-cell", "{display_title}" }
                if let Some(err) = book.error.as_ref() {
                    div { class: "error", "⚠ {err}" }
                }
            }
            td { class: "ebook-col-author", "{authors}" }
            td { class: "ebook-col-series", "{series_line}" }
            td { class: "ebook-col-publisher", {book.publisher.as_deref().unwrap_or("")} }
            td { class: "ebook-col-published", {book.published.as_deref().unwrap_or("")} }
            td { class: "ebook-col-language", {book.language.as_deref().unwrap_or("")} }
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
