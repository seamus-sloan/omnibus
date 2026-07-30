//! Quote-card drawer. Thin reader shell (scrim + right-hand drawer) around
//! the shared [`crate::components::QuoteCardPanel`] editor; the PNG export is
//! drawn by the standalone `quote-card.js` the reader loads alongside its
//! glue.

use dioxus::prelude::*;

use crate::components::QuoteCardPanel;

#[component]
pub(super) fn QuotePanel(
    quote_text: String,
    author: String,
    subtitle: String,
    accent: String,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer rd-quote-drawer", "data-testid": "reader-quote-drawer",
            div { class: "rd-grabber" }
            QuoteCardPanel {
                quote_text,
                author,
                subtitle,
                accent,
                on_close,
            }
        }
    }
}
