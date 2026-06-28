//! Highlights & notes drawer for the reader. Lists every highlight for the
//! open book, filterable by palette color. Clicking a row navigates to the
//! highlight's CFI; rows also expose copy and delete actions.

use dioxus::prelude::*;

use omnibus_shared::{Highlight, HighlightColor};

const PALETTE: [(HighlightColor, &str); 5] = [
    (HighlightColor::Amber, "amber"),
    (HighlightColor::Green, "green"),
    (HighlightColor::Blue, "blue"),
    (HighlightColor::Rose, "rose"),
    (HighlightColor::Violet, "violet"),
];

/// Navigate the rendition to a CFI (web only).
#[cfg_attr(not(feature = "web"), allow(unused_variables))]
fn navigate_to(cfi: &str) {
    #[cfg(feature = "web")]
    {
        let lit = serde_json::to_string(cfi).unwrap_or_else(|_| "\"\"".into());
        super::reader_call("display", &lit);
    }
}

/// Copy text to the clipboard via the glue (web only).
#[cfg_attr(not(feature = "web"), allow(unused_variables))]
fn copy_text(text: &str) {
    #[cfg(feature = "web")]
    {
        let lit = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
        super::reader_call("copyText", &lit);
    }
}

#[component]
pub(super) fn HighlightsDrawer(
    highlights: Signal<Vec<Highlight>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut filter = use_signal(|| None::<HighlightColor>);

    let all = highlights.read().clone();
    let active = filter();
    let shown: Vec<Highlight> = all
        .iter()
        .filter(|h| active.is_none() || active == Some(h.color))
        .cloned()
        .collect();

    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer", "data-testid": "reader-highlights-drawer",
            div { class: "rd-drawer-head",
                h4 { class: "rd-drawer-title",
                    "Highlights & notes "
                    span { class: "rd-drawer-count", "{all.len()}" }
                }
                button {
                    class: "rd-x",
                    r#type: "button",
                    "aria-label": "Close",
                    onclick: move |_| on_close.call(()),
                    "\u{00d7}"
                }
            }
            div { class: "rd-filter-row",
                button {
                    class: if active.is_none() { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    onclick: move |_| filter.set(None),
                    "All {all.len()}"
                }
                for (color, name) in PALETTE {
                    button {
                        key: "{name}",
                        class: if active == Some(color) { "rd-chip on" } else { "rd-chip" },
                        r#type: "button",
                        "aria-label": "Filter {name}",
                        onclick: move |_| filter.set(Some(color)),
                        span { class: "rd-chip-dot", "data-color": "{name}" }
                    }
                }
            }
            div { class: "rd-drawer-body",
                if shown.is_empty() {
                    div { class: "rd-drawer-empty", "No highlights yet." }
                } else {
                    for h in shown.iter() {
                        HighlightRow { key: "{h.id}", highlight: h.clone(), highlights }
                    }
                }
            }
        }
    }
}

#[component]
fn HighlightRow(highlight: Highlight, highlights: Signal<Vec<Highlight>>) -> Element {
    let color = highlight.color.as_str();
    let quote = highlight
        .text
        .clone()
        .unwrap_or_else(|| "(highlighted passage)".to_string());
    let note = highlight.note.clone();
    let cfi = highlight.epub_cfi_range.clone();
    let copy_src = highlight.text.clone().unwrap_or_default();
    let id = highlight.id;

    let on_delete = move |_| {
        let mut highlights = highlights;
        let cfi = cfi.clone();
        spawn(async move {
            if crate::data::delete_highlight("", id).await.is_ok() {
                #[cfg(feature = "web")]
                {
                    let lit = serde_json::to_string(&cfi).unwrap_or_else(|_| "\"\"".into());
                    super::reader_call("removeAnnotation", &lit);
                }
                let _ = &cfi;
                highlights.write().retain(|h| h.id != id);
            }
        });
    };

    let nav_cfi = highlight.epub_cfi_range.clone();

    rsx! {
        div { class: "rd-hl-row", "data-testid": "reader-highlight-row",
            style: "border-left-color: var(--hl-{color});",
            button {
                class: "rd-hl-quote",
                r#type: "button",
                onclick: move |_| navigate_to(&nav_cfi),
                "\u{201c}{quote}\u{201d}"
            }
            if let Some(n) = note {
                div { class: "rd-hl-note", "{n}" }
            }
            div { class: "rd-hl-actions",
                button {
                    class: "rd-act sm",
                    r#type: "button",
                    onclick: move |_| copy_text(&copy_src),
                    "Copy"
                }
                button {
                    class: "rd-act sm",
                    r#type: "button",
                    "data-testid": "highlight-delete",
                    onclick: on_delete,
                    "Delete"
                }
            }
        }
    }
}
