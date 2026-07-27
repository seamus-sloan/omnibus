//! "Passages you saved" — the current user's highlights for this book, listed
//! outside the reader. Each row carries the quote, its note, a CFI-derived
//! locator, and the date it was saved, plus open-in-reader / copy / delete
//! actions mirroring the reader's highlights drawer. Entries attach post-mount
//! so SSR and first-hydration paint stay identical (rule 07).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::Highlight;

use crate::{data, use_server_url};

use super::dates::fmt_long_date;
use super::BdSectionHead;

mod locator;

use locator::highlight_locator;

#[cfg(test)]
mod tests;

/// Saved-passages section: header, then the highlight list or an empty note.
#[component]
pub(super) fn BdHighlightsSection(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut highlights = use_signal(Vec::<Highlight>::new);

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
                div { class: "bd-hl-list", "data-testid": "highlights-list",
                    for h in list.iter() {
                        BdHighlightCard {
                            key: "{h.id}",
                            highlight: h.clone(),
                            highlights,
                            server_url: server_url.clone(),
                        }
                    }
                }
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

/// One saved passage: the quote, its note, the locator/date meta line, and the
/// open / copy / delete actions. Delete drops the row optimistically only
/// after the server confirms, so a failed request leaves the list intact.
#[component]
fn BdHighlightCard(
    highlight: Highlight,
    highlights: Signal<Vec<Highlight>>,
    server_url: String,
) -> Element {
    let id = highlight.id;
    let color = highlight.color.as_str();
    let quote = highlight
        .text
        .clone()
        .unwrap_or_else(|| "(highlighted passage)".to_string());
    // Highlights created before the text column (migration 0030) have nothing
    // to copy; disable the action rather than offer a silent no-op.
    let copy_src = highlight.text.clone();
    let note = highlight.note.clone();
    let meta_line = match highlight_locator(&highlight.epub_cfi_range) {
        Some(loc) => format!(
            "{loc} \u{00b7} saved {}",
            fmt_long_date(highlight.created_at)
        ),
        None => format!("saved {}", fmt_long_date(highlight.created_at)),
    };
    let open_href = reader_deep_link(&highlight.book_uuid, &highlight.epub_cfi_range);

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
            div { class: "bd-hl-foot",
                span { class: "mono bd-hl-meta", "data-testid": "highlight-meta", "{meta_line}" }
                div { class: "bd-hl-actions",
                    Link {
                        to: "{open_href}",
                        class: "btn ghost sm",
                        "data-testid": "highlight-open",
                        "Open in reader"
                    }
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        "data-testid": "highlight-copy",
                        disabled: copy_src.is_none(),
                        onclick: move |_| {
                            if let Some(ref text) = copy_src {
                                copy_to_clipboard(text);
                            }
                        },
                        "Copy"
                    }
                    button {
                        class: "btn ghost sm bd-hl-delete",
                        r#type: "button",
                        "data-testid": "highlight-delete",
                        onclick: on_delete,
                        "Delete"
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
