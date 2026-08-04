//! Table-of-contents drawer for the reader. The flat TOC arrives from the
//! epub.js glue via the `__omnibusOnToc` callback (registered in `interop`);
//! clicking an entry navigates the rendition to that href.

use dioxus::prelude::*;

use super::drawer_shell::ReaderDrawerShell;

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
        ReaderDrawerShell {
            testid: "reader-toc-drawer",
            on_close,
            head: rsx! {
                h4 { class: "rd-drawer-title", "Contents" }
                if !progress_label.is_empty() {
                    span { class: "rd-drawer-sub", "{progress_label}" }
                }
            },
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

// SSR render-smoke coverage. The two `EventHandler`s must be constructed in a
// component body, so the tests mount the drawer through a prop-only harness.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render;

    #[component]
    fn TocHarness(
        entries: Vec<TocEntry>,
        current_title: String,
        progress_label: String,
    ) -> Element {
        rsx! {
            TocDrawer {
                entries,
                current_title,
                progress_label,
                on_navigate: move |_| {},
                on_close: move |_| {},
            }
        }
    }

    fn entry(label: &str, href: &str) -> TocEntry {
        TocEntry {
            label: label.to_string(),
            href: href.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn toc_drawer_lists_each_entry_and_marks_the_current_chapter() {
        let html = render(rsx! {
            TocHarness {
                entries: vec![entry("Chapter One", "ch1.xhtml"), entry("Chapter Two", "ch2.xhtml")],
                current_title: "Chapter Two".to_string(),
                progress_label: "184 / 272 \u{00b7} 68%".to_string(),
            }
        });

        assert!(html.contains("data-testid=\"reader-toc-drawer\""));
        assert!(html.contains("Contents"));
        assert!(html.contains("aria-label=\"Close\""));
        assert!(html.contains("data-testid=\"reader-toc-row\""));
        assert!(html.contains("Chapter One"));
        assert!(html.contains("Chapter Two"));
        // The current chapter's row takes the highlight class, and the phone
        // sheet's progress line renders when non-empty.
        assert!(html.contains("rd-toc-row current"));
        assert!(html.contains("184 / 272 \u{00b7} 68%"));
    }

    #[test]
    fn toc_drawer_shows_an_empty_state_without_entries() {
        let html = render(rsx! {
            TocHarness {
                entries: vec![],
                current_title: String::new(),
                progress_label: String::new(),
            }
        });

        assert!(html.contains("No table of contents."));
        assert!(!html.contains("data-testid=\"reader-toc-row\""));
    }
}
