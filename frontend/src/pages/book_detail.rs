use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::{data, use_server_url, Route};

#[component]
pub fn BookDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_ebook(&url, id).await {
                Ok(b) => {
                    book.set(Some(b));
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
    let Some(b) = book() else {
        return rsx! {
            p { class: "subtitle", "Book not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    };

    let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    let authors = b
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let breadcrumb_mid = b
        .series
        .clone()
        .or_else(|| b.creators.first().map(|c| c.name.clone()))
        .unwrap_or_default();
    let series_label = match (&b.series, &b.series_index) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.clone()),
        _ => None,
    };
    let cover_src = b.cover_url.as_ref().map(|u| format!("{server_url}{u}"));
    let has_epub = b.formats.iter().any(|f| f.eq_ignore_ascii_case("epub"));
    let has_m4b = b.formats.iter().any(|f| f.eq_ignore_ascii_case("m4b"));

    rsx! {
        nav { class: "breadcrumb", aria_label: "breadcrumb",
            Link { to: Route::Landing {}, "Home" }
            if !breadcrumb_mid.is_empty() {
                span { " \u{203a} " }
                span { "{breadcrumb_mid}" }
            }
            span { " \u{203a} " }
            span { "{title}" }
        }

        div { class: "book-detail",
            div { class: "book-detail-cover",
                if let Some(src) = cover_src {
                    img {
                        src: "{src}",
                        alt: "Cover of {title}",
                        loading: "lazy",
                    }
                } else {
                    div { class: "book-detail-cover-fallback", "\u{1F4D6}" }
                }
            }

            div { class: "book-detail-meta",
                h1 { "{title}" }
                if !authors.is_empty() {
                    p { "{authors}" }
                }
                if let Some(s) = series_label {
                    p { class: "subtitle", "{s}" }
                }
                if let Some(desc) = &b.description {
                    p { class: "book-detail-description", "{desc}" }
                }

                if has_epub || has_m4b {
                    div { class: "format-actions",
                        if has_epub {
                            button {
                                class: "btn",
                                disabled: true,
                                title: "Reader coming soon",
                                "data-testid": "action-read",
                                "Read"
                            }
                            button {
                                class: "btn",
                                disabled: true,
                                title: "Send-to-Kindle coming soon",
                                "data-testid": "action-kindle",
                                "Send to Kindle"
                            }
                        }
                        if has_m4b {
                            button {
                                class: "btn",
                                disabled: true,
                                title: "Audio player coming soon",
                                "data-testid": "action-listen",
                                "Listen"
                            }
                        }
                    }
                }

                if !b.subjects.is_empty() {
                    ul { class: "tag-list",
                        for tag in &b.subjects {
                            li { class: "tag", "{tag}" }
                        }
                    }
                }

                if !b.identifiers.is_empty() {
                    dl { class: "identifier-list",
                        for ident in &b.identifiers {
                            dt { "{ident.scheme.as_deref().unwrap_or(\"\")}" }
                            dd { "{ident.value}" }
                        }
                    }
                }

                if let Some(pub_) = &b.publisher {
                    p { class: "subtitle", "Publisher: {pub_}" }
                }
                if let Some(lang) = &b.language {
                    p { class: "subtitle", "Language: {lang}" }
                }
                if let Some(date) = &b.published {
                    p { class: "subtitle", "Published: {date}" }
                }

                div {
                    "data-testid": "ratings-slot",
                    aria_label: "Ratings \u{2014} coming soon",
                }
                div {
                    "data-testid": "suggestions-slot",
                    aria_label: "Suggestions \u{2014} coming soon",
                }

                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}
