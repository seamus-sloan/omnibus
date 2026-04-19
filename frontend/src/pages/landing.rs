use dioxus::prelude::*;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata};

use crate::{data, use_server_url};

/// Landing page — loads the configured ebook library and renders every book
/// in a single table with cover thumbnails and the common metadata columns.
/// Rows are clickable but currently have no destination (book detail page
/// is a TODO).
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
                            th { "Title" }
                            th { "Author" }
                            th { "Series" }
                            th { "Publisher" }
                            th { "Published" }
                            th { "Language" }
                        }
                    }
                    tbody {
                        for book in lib.books {
                            EbookRow { key: "{book.filename}", book: book }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EbookRow(book: EbookMetadata) -> Element {
    let display_title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let series_line = match (book.series.as_ref(), book.series_index.as_ref()) {
        (Some(s), Some(i)) => format!("{s} #{i}"),
        (Some(s), None) => s.clone(),
        _ => String::new(),
    };
    let authors = contributor_names(&book.creators);

    rsx! {
        tr {
            class: "ebook-row",
            "data-testid": "ebook-row",
            // TODO: navigate to a book-detail page once it exists.
            onclick: move |_| {},
            td { class: "ebook-col-cover",
                if let Some(src) = book.cover_image.as_ref() {
                    img { class: "ebook-thumb", src: "{src}", alt: "Cover of {display_title}" }
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
            td { "{authors}" }
            td { "{series_line}" }
            td { {book.publisher.clone().unwrap_or_default()} }
            td { {book.published.clone().unwrap_or_default()} }
            td { {book.language.clone().unwrap_or_default()} }
        }
    }
}

fn contributor_names(list: &[Contributor]) -> String {
    list.iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}
