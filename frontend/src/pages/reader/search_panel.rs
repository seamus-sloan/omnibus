//! In-book search panel. The query is run by the epub.js glue's `search`
//! method (spine walk + `section.find`), which posts matches back through
//! the `__omnibusOnSearchResults` callback registered in `interop`.

use dioxus::prelude::*;

/// One search match from the glue.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
pub(crate) struct SearchResult {
    pub cfi: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub chapter: String,
}

#[component]
pub(super) fn SearchPanel(
    results: Signal<Vec<SearchResult>>,
    on_query: EventHandler<String>,
    on_navigate: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut query = use_signal(String::new);
    let hits = results.read().clone();
    let has_query = !query.read().trim().is_empty();

    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        // `rd-search-drawer` lets the phone breakpoint take this one drawer
        // full-screen while the rest stay bottom sheets.
        div { class: "rd-drawer rd-search-drawer", "data-testid": "reader-search-drawer",
            div { class: "rd-grabber" }
            div { class: "rd-drawer-head",
                h4 { class: "rd-drawer-title", "Search" }
                button {
                    class: "rd-x",
                    r#type: "button",
                    "aria-label": "Close",
                    onclick: move |_| on_close.call(()),
                    "\u{00d7}"
                }
            }
            div { class: "rd-search-box",
                input {
                    class: "rd-search-input",
                    r#type: "text",
                    "data-testid": "reader-search-input",
                    placeholder: "Search in book\u{2026} (Enter)",
                    autofocus: true,
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            on_query.call(query.peek().trim().to_string());
                        }
                    },
                }
            }
            if hits.len() == 1 {
                div { class: "rd-search-count", "1 match \u{b7} in this book" }
            } else if !hits.is_empty() {
                div { class: "rd-search-count", "{hits.len()} matches \u{b7} in this book" }
            }
            div { class: "rd-drawer-body",
                if hits.is_empty() {
                    div { class: "rd-drawer-empty",
                        if has_query { "No matches." } else { "Type a query and press Enter." }
                    }
                } else {
                    for hit in hits.iter() {
                        {
                            let cfi = hit.cfi.clone();
                            rsx! {
                                button {
                                    key: "{hit.cfi}",
                                    class: "rd-search-row",
                                    r#type: "button",
                                    "data-testid": "reader-search-row",
                                    onclick: move |_| on_navigate.call(cfi.clone()),
                                    if !hit.chapter.is_empty() {
                                        span { class: "rd-search-chapter", "{hit.chapter}" }
                                    }
                                    span { class: "rd-search-excerpt", "{hit.excerpt}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
