//! "Passages you saved" — the current user's highlights for this book, listed
//! outside the reader. Each row carries the quote, note, CFI-derived locator,
//! and saved date, plus open-in-reader / quote / copy / delete actions
//! mirroring the reader's drawer (Quote opens the shared editor in a modal).
//! Entries attach post-mount so SSR and first hydration match (rule 07).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::Highlight;

use crate::components::quote_card::QUOTE_CARD_JS;
use crate::components::{ConfirmModal, QuoteCardPanel};
use crate::{data, use_server_url};

use super::dates::{fmt_long_date, local_date_offset, use_local_dates_ready};
use super::BdSectionHead;

mod locator;

use locator::highlight_locator;

#[cfg(test)]
mod tests;

/// Book identity the quote-card editor stamps onto the exported card.
#[derive(Clone, PartialEq, Props)]
pub(super) struct BdQuoteMeta {
    pub title: String,
    pub author: String,
    pub accent: Option<String>,
}

/// Saved-passages section: header, then the highlight list or an empty note.
#[component]
pub(super) fn BdHighlightsSection(uuid: String, quote_meta: BdQuoteMeta) -> Element {
    let server_url = use_server_url();
    let mut highlights = use_signal(Vec::<Highlight>::new);
    // The passage targeted for a quote card; `None` on SSR and first paint
    // (rule 07), set by each card's Quote button.
    let quote_target: Signal<Option<Highlight>> = use_signal(|| None);

    let load_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(list) = data::list_highlights(&url, &uuid).await {
                highlights.set(list);
            }
        });
    }));

    let list = highlights();
    rsx! {
        // The standalone canvas renderer the quote-card modal's export
        // actions call into (`window.OmnibusQuoteCard`).
        document::Script { src: QUOTE_CARD_JS }
        div { class: "bd-highlights", "data-testid": "highlights-section",
            BdSectionHead {
                kicker: passages_kicker(list.len()),
                title: "Passages you saved".to_string(),
            }
            if list.is_empty() {
                div { class: "bd-journal-empty card", "data-testid": "highlights-empty",
                    p { class: "mono",
                        "No saved passages yet \u{2014} highlight while you read to keep them here."
                    }
                }
            } else {
                BdHighlightList {
                    highlights,
                    server_url: server_url.clone(),
                    quote_target,
                }
            }
        }
        {render_quote_modal(quote_target, quote_meta)}
    }
}

/// Passages listed before the reader expands the section. A heavily
/// highlighted book runs to hundreds of rows, and this is one section of
/// several on the page.
const COLLAPSED_PASSAGES: usize = 5;

/// The passage list, capped at [`COLLAPSED_PASSAGES`] rows until the reader
/// expands it. Both renders start collapsed so SSR and hydration match
/// (rule 07).
#[component]
fn BdHighlightList(
    highlights: Signal<Vec<Highlight>>,
    server_url: String,
    quote_target: Signal<Option<Highlight>>,
) -> Element {
    let mut expanded = use_signal(|| false);
    // Hoisted once for the whole list rather than once per card — a heavily
    // highlighted book runs to hundreds of rows, and each row resolves its
    // own offset from its own `created_at` via `local_date_offset` (see
    // `dates.rs`), so sharing this signal doesn't share a stale offset.
    let dates_ready = use_local_dates_ready();
    let list = highlights();
    let total = list.len();
    let visible = if expanded() {
        total
    } else {
        total.min(COLLAPSED_PASSAGES)
    };

    rsx! {
        div { class: "bd-hl-list", "data-testid": "highlights-list",
            for h in list.iter().take(visible) {
                BdHighlightCard {
                    key: "{h.id}",
                    highlight: h.clone(),
                    highlights,
                    server_url: server_url.clone(),
                    quote_target,
                    dates_ready,
                }
            }
        }
        if total > visible {
            button {
                r#type: "button",
                class: "btn ghost sm bd-hl-show-more",
                "data-testid": "highlights-show-more",
                "aria-expanded": "{expanded()}",
                onclick: move |_| expanded.set(true),
                "Show {total - visible} more \u{2193}"
            }
        }
    }
}

/// Quote-card modal, shown while a passage is targeted for a quote card.
/// The shared editor sits in the app's modal shell rather than the reader's
/// right-hand drawer (whose absolute positioning assumes the reader surface).
fn render_quote_modal(quote_target: Signal<Option<Highlight>>, meta: BdQuoteMeta) -> Element {
    let Some(h) = quote_target.read().clone() else {
        return rsx! {};
    };
    let close = move |_| {
        let mut quote_target = quote_target;
        quote_target.set(None);
    };
    rsx! {
        ConfirmModal {
            testid: "quote-card-modal".to_string(),
            aria_label: "Make a quote card".to_string(),
            dialog_class: "mg-modal bd-quote-modal".to_string(),
            busy: false,
            on_dismiss: close,
            QuoteCardPanel {
                quote_text: h.text.clone().unwrap_or_default(),
                author: meta.author,
                subtitle: meta.title,
                // Same fallback the reader uses when a book carries no accent.
                accent: meta.accent.unwrap_or_else(|| "#3a3027".to_string()),
                on_close: close,
            }
        }
    }
}

/// Section kicker: "Quotes · none saved" or "Quotes · N saved passage(s)".
fn passages_kicker(count: usize) -> String {
    if count == 0 {
        return "Quotes \u{00b7} none saved".to_string();
    }
    let word = if count == 1 { "passage" } else { "passages" };
    format!("Quotes \u{00b7} {count} saved {word}")
}

/// One saved passage: the quote, its note, the locator/date meta line, and
/// the open / quote / copy / delete actions. Delete drops the row
/// optimistically only after the server confirms, so a failed request leaves
/// the list intact.
#[component]
fn BdHighlightCard(
    highlight: Highlight,
    highlights: Signal<Vec<Highlight>>,
    server_url: String,
    quote_target: Signal<Option<Highlight>>,
    dates_ready: ReadSignal<bool>,
) -> Element {
    let offset = local_date_offset(dates_ready(), highlight.created_at);
    let id = highlight.id;
    let color = highlight.color.as_str();
    let quote = highlight
        .text
        .clone()
        .unwrap_or_else(|| "(highlighted passage)".to_string());
    // Highlights created before the text column (migration 0030) have nothing
    // to copy or put on a card; disable both actions rather than offer a
    // silent no-op.
    let copy_src = highlight.text.clone();
    let quote_disabled = highlight.text.is_none();
    let quote_src = highlight.clone();
    let note = highlight.note.clone();
    // A Kobo-origin passage has no CFI: it still lists, but there is no
    // position to derive a locator from and nowhere for "Open in reader" to
    // jump.
    let meta_line = match highlight
        .epub_cfi_range
        .as_deref()
        .and_then(highlight_locator)
    {
        Some(loc) => format!(
            "{loc} \u{00b7} saved {}",
            fmt_long_date(highlight.created_at, offset)
        ),
        None => format!("saved {}", fmt_long_date(highlight.created_at, offset)),
    };
    let open_href = highlight
        .epub_cfi_range
        .as_deref()
        .map(|cfi| reader_deep_link(&highlight.book_uuid, cfi));

    let on_delete = move |_| {
        let mut highlights = highlights;
        let url = server_url.clone();
        spawn(async move {
            if data::delete_highlight(&url, id).await.is_ok() {
                highlights.write().retain(|h| h.id != id);
            }
        });
    };

    rsx! {
        article {
            class: "card bd-hl-card",
            "data-testid": "highlight-card",
            "data-color": "{color}",
            style: "border-left-color: var(--hl-{color});",
            blockquote { class: "bd-hl-quote", "\u{201c}{quote}\u{201d}" }
            if let Some(n) = note {
                p { class: "bd-hl-note", "data-testid": "highlight-note", "{n}" }
            }
            // One mono line, per the W4 design: locator · saved date · the
            // actions as inline text links rather than a button row.
            div { class: "bd-hl-foot",
                span { class: "mono bd-hl-meta", "data-testid": "highlight-meta", "{meta_line}" }
                div { class: "bd-hl-actions",
                    if let Some(href) = open_href {
                        Link {
                            to: "{href}",
                            class: "bd-hl-act",
                            "data-testid": "highlight-open",
                            "open in book \u{2192}"
                        }
                    }
                    button {
                        class: "bd-hl-act",
                        r#type: "button",
                        "data-testid": "highlight-quote",
                        disabled: quote_disabled,
                        onclick: move |_| {
                            let mut quote_target = quote_target;
                            quote_target.set(Some(quote_src.clone()));
                        },
                        "quote card \u{2192}"
                    }
                    button {
                        class: "bd-hl-act",
                        r#type: "button",
                        "data-testid": "highlight-copy",
                        disabled: copy_src.is_none(),
                        onclick: move |_| {
                            if let Some(ref text) = copy_src {
                                copy_to_clipboard(text);
                            }
                        },
                        "copy"
                    }
                    button {
                        class: "bd-hl-act bd-hl-delete",
                        r#type: "button",
                        "data-testid": "highlight-delete",
                        onclick: on_delete,
                        "delete"
                    }
                }
            }
        }
    }
}

/// Reader URL that opens `book_uuid` at `cfi`. The reader prefers a `?cfi=`
/// deep link over saved progress, so the link lands on the passage rather than
/// wherever this book was last left off.
fn reader_deep_link(book_uuid: &str, cfi: &str) -> String {
    format!(
        "/read/{}?cfi={}",
        percent_encode(book_uuid),
        percent_encode(cfi)
    )
}

/// Percent-encode a path/query segment. Hand-rolled rather than pulled from a
/// crate because the only inputs are a uuid and an EPUB CFI, and the CFI's
/// `/`, `[`, `,`, `:` and `!` all need escaping inside a query value.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Write `text` to the system clipboard (no-op on SSR, where there is none).
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(unused_variables))]
fn copy_to_clipboard(text: &str) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    {
        let lit = crate::js_interop::json_literal(text);
        let _ = dioxus::document::eval(&format!(
            "navigator.clipboard && navigator.clipboard.writeText({lit});"
        ));
    }
}
