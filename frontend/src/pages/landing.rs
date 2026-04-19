use dioxus::prelude::*;
use omnibus_shared::{EbookLibrary, EbookMetadata};

use crate::{data, use_server_url};

/// Landing page — loads the configured ebook library and renders each book
/// with its cover art and parsed OPF metadata.
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

    rsx! {
        section { class: "card",
            h1 { "Your Library" }
            p { class: "subtitle",
                if let Some(path) = library().path.as_ref() {
                    "{path}"
                } else {
                    "Configure your ebook library path in Settings."
                }
            }
            if let Some(msg) = error() {
                p { class: "error", "⚠ {msg}" }
            }
            if let Some(msg) = library().error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
        }

        div {
            id: "ebook-grid",
            "data-testid": "ebook-grid",
            class: "ebook-grid",
            if loading() {
                p { class: "library-empty", "Loading..." }
            } else if library().books.is_empty() && library().error.is_none() && error().is_none() {
                p { class: "library-empty", "No ebooks found." }
            } else {
                for book in library().books {
                    EbookCard { key: "{book.filename}", book: book }
                }
            }
        }
    }
}

#[component]
fn EbookCard(book: EbookMetadata) -> Element {
    let display_title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let authors = if book.authors.is_empty() {
        None
    } else {
        Some(book.authors.join(", "))
    };
    let subjects = if book.subjects.is_empty() {
        None
    } else {
        Some(book.subjects.join(" · "))
    };
    let series_line = match (book.series.as_ref(), book.series_index.as_ref()) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.clone()),
        _ => None,
    };

    rsx! {
        article { class: "ebook-card", "data-testid": "ebook-card",
            div { class: "ebook-cover",
                if let Some(src) = book.cover_image.as_ref() {
                    img { src: "{src}", alt: "Cover of {display_title}" }
                } else {
                    div { class: "ebook-cover-fallback", "No cover" }
                }
            }
            div { class: "ebook-info",
                h2 { class: "ebook-title", "{display_title}" }
                if let Some(a) = authors {
                    p { class: "ebook-authors", "by {a}" }
                }
                if let Some(s) = series_line {
                    p { class: "ebook-series", "Series: {s}" }
                }
                if let Some(pub_) = book.publisher.as_ref() {
                    p { class: "ebook-meta", "Publisher: {pub_}" }
                }
                if let Some(d) = book.published.as_ref() {
                    p { class: "ebook-meta", "Published: {d}" }
                }
                if let Some(l) = book.language.as_ref() {
                    p { class: "ebook-meta", "Language: {l}" }
                }
                if let Some(id) = book.identifier.as_ref() {
                    p { class: "ebook-meta", "Identifier: {id}" }
                }
                if let Some(s) = subjects {
                    p { class: "ebook-subjects", "{s}" }
                }
                if let Some(desc) = book.description.as_ref() {
                    // Descriptions frequently ship as raw HTML fragments — keep
                    // as plain text for safety; the card already constrains the
                    // height via CSS.
                    p { class: "ebook-description", "{strip_html(desc)}" }
                }
                if let Some(err) = book.error.as_ref() {
                    p { class: "error", "⚠ {err} ({book.filename})" }
                }
            }
        }
    }
}

/// Extremely cheap HTML-tag stripper. EPUB descriptions are frequently raw
/// HTML; we render as text to avoid injecting arbitrary markup into the page.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
