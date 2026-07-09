//! Table-of-contents drawer for the reader. The flat TOC arrives from the
//! epub.js glue via the `__omnibusOnToc` callback (registered in `interop`);
//! clicking an entry navigates the rendition to that href.

use dioxus::prelude::*;

/// One flattened table-of-contents entry from the glue. `level` is the
/// nesting depth (0 = top) used to indent nested chapters.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
pub(crate) struct TocEntry {
    pub label: String,
    pub href: String,
    #[serde(default)]
    pub level: u32,
}

/// Right-side contents drawer (bottom sheet on phones). `current_title`
/// highlights the entry matching the reader's current chapter;
/// `progress_label` is the phone sheet's "184 / 272 · 68%" line.
#[component]
pub(super) fn TocDrawer(
    entries: Vec<TocEntry>,
    current_title: String,
    progress_label: String,
    on_navigate: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer", "data-testid": "reader-toc-drawer",
            div { class: "rd-grabber" }
            div { class: "rd-drawer-head",
                h4 { class: "rd-drawer-title", "Contents" }
                if !progress_label.is_empty() {
                    span { class: "rd-drawer-sub", "{progress_label}" }
                }
                button {
                    class: "rd-x",
                    r#type: "button",
                    "aria-label": "Close",
                    onclick: move |_| on_close.call(()),
                    "\u{00d7}"
                }
            }
            div { class: "rd-drawer-body",
                if entries.is_empty() {
                    div { class: "rd-drawer-empty", "No table of contents." }
                } else {
                    for entry in entries.iter() {
                        {
                            let is_current = !current_title.is_empty()
                                && entry.label == current_title;
                            let href = entry.href.clone();
                            let row_class = if is_current {
                                "rd-toc-row current"
                            } else {
                                "rd-toc-row"
                            };
                            let indent = format!("padding-left:{}px;", 14 + entry.level * 16);
                            rsx! {
                                button {
                                    key: "{entry.href}",
                                    class: "{row_class}",
                                    style: "{indent}",
                                    r#type: "button",
                                    "data-testid": "reader-toc-row",
                                    onclick: move |_| on_navigate.call(href.clone()),
                                    "{entry.label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
