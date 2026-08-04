//! Highlights & notes drawer for the reader. Lists every highlight for the
//! open book, filterable by palette color. Clicking a row navigates to the
//! highlight's CFI; rows expose a recolor swatch strip plus note, quote,
//! copy, and delete actions.

use dioxus::prelude::*;
use omnibus_shared::{Highlight, HighlightColor};

use super::drawer_shell::ReaderDrawerShell;

#[cfg(test)]
mod tests;

pub(super) const PALETTE: [(HighlightColor, &str); 5] = [
    (HighlightColor::Amber, "amber"),
    (HighlightColor::Green, "green"),
    (HighlightColor::Blue, "blue"),
    (HighlightColor::Rose, "rose"),
    (HighlightColor::Violet, "violet"),
];

/// Navigate the rendition to a CFI (via the glue; SSR no-op).
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(unused_variables))]
fn navigate_to(cfi: &str) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    super::reader_call_json("display", cfi);
}

/// Copy text to the clipboard via the glue (SSR no-op).
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(unused_variables))]
fn copy_text(text: &str) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    super::reader_call_json("copyText", text);
}

/// Repaint a highlight's annotation in place: drop the old swatch and re-add
/// it at the same CFI in `color` (via the glue; SSR no-op). Used when
/// recoloring so the viewer reflects the new color without a full reload.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(unused_variables))]
fn reannotate(cfi: &str, color: HighlightColor) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    {
        super::reader_call_json("removeAnnotation", cfi);
        super::reader_call_json2("addAnnotation", cfi, color.as_str());
    }
}

/// Recolor a highlight to `next`: optimistically repaint the viewer + the row
/// signal, persist via `update_highlight_color`, and roll both back to `prev`
/// if the write fails. Mirrors the create/delete optimistic pattern.
///
/// A no-op when `next == prev` (clicking the already-selected swatch). The
/// rollback is gated on the row still showing `next`, so a late failure from
/// one request can't clobber a newer recolor that already succeeded.
fn spawn_recolor(
    server_url: String,
    mut highlights: Signal<Vec<Highlight>>,
    id: i64,
    cfi: Option<String>,
    next: HighlightColor,
    prev: HighlightColor,
) {
    if next == prev {
        return;
    }
    // A Kobo-origin highlight has no CFI, so there is nothing painted in the
    // viewer to repaint — the row color and the persisted value still change.
    if let Some(cfi) = &cfi {
        reannotate(cfi, next);
    }
    // Bind the index first so the read guard drops before `write()` — a live
    // `read()` temporary across `write()` is a runtime borrow panic.
    let idx = highlights.read().iter().position(|h| h.id == id);
    if let Some(i) = idx {
        highlights.write()[i].color = next;
    }
    spawn(async move {
        if crate::data::update_highlight_color(&server_url, id, next)
            .await
            .is_err()
        {
            // Only revert if this request's optimistic color is still current;
            // if a later recolor superseded it, leave that newer value alone.
            let still_ours = highlights
                .read()
                .iter()
                .any(|h| h.id == id && h.color == next);
            if still_ours {
                if let Some(cfi) = &cfi {
                    reannotate(cfi, prev);
                }
                let idx = highlights.read().iter().position(|h| h.id == id);
                if let Some(i) = idx {
                    highlights.write()[i].color = prev;
                }
            }
        }
    });
}

#[component]
pub(super) fn HighlightsDrawer(
    highlights: Signal<Vec<Highlight>>,
    on_quote: EventHandler<Highlight>,
    on_edit_note: EventHandler<Highlight>,
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
        ReaderDrawerShell {
            testid: "reader-highlights-drawer",
            on_close,
            head: rsx! {
                h4 { class: "rd-drawer-title",
                    "Highlights & notes "
                    span { class: "rd-drawer-count", "{all.len()}" }
                }
            },
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
                        HighlightRow {
                            key: "{h.id}",
                            highlight: h.clone(),
                            highlights,
                            on_quote,
                            on_edit_note,
                        }
                    }
                }
            }
        }
    }
}

/// Build the delete handler: removes the persisted highlight, then its
/// painted annotation (if any) and the row from `highlights`.
fn build_delete_handler(
    server_url: String,
    mut highlights: Signal<Vec<Highlight>>,
    id: i64,
    cfi: Option<String>,
) -> impl FnMut(MouseEvent) + 'static {
    move |_| {
        let cfi = cfi.clone();
        let server_url = server_url.clone();
        spawn(async move {
            if crate::data::delete_highlight(&server_url, id).await.is_ok() {
                // Anchorless (Kobo-origin) rows have no painted swatch to
                // remove from the rendition.
                if let Some(cfi) = &cfi {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("removeAnnotation", cfi);
                    let _ = cfi;
                }
                highlights.write().retain(|h| h.id != id);
            }
        });
    }
}

/// Recolor swatch strip: one dot per palette color, highlighting the current
/// one and dispatching [`spawn_recolor`] on click.
fn hl_swatch_strip(
    cur_color: HighlightColor,
    recolor_cfi: Option<String>,
    server_url: String,
    highlights: Signal<Vec<Highlight>>,
    id: i64,
) -> Element {
    rsx! {
        div { class: "rd-hl-swatches", "data-testid": "reader-highlight-swatches",
            for (swatch_color, swatch_name) in PALETTE {
                button {
                    key: "{swatch_name}",
                    class: if swatch_color == cur_color { "rd-hl-swatch on" } else { "rd-hl-swatch" },
                    r#type: "button",
                    "data-color": "{swatch_name}",
                    "data-testid": "reader-highlight-recolor-{swatch_name}",
                    "aria-label": "Recolor {swatch_name}",
                    "aria-pressed": if swatch_color == cur_color { "true" } else { "false" },
                    onclick: {
                        let recolor_cfi = recolor_cfi.clone();
                        let server_url = server_url.clone();
                        move |_| spawn_recolor(server_url.clone(), highlights, id, recolor_cfi.clone(), swatch_color, cur_color)
                    },
                }
            }
        }
    }
}

/// Note/quote/copy/delete action row.
fn hl_actions_row(
    highlight: &Highlight,
    note_label: &str,
    has_text: bool,
    copy_src: String,
    on_quote: EventHandler<Highlight>,
    on_edit_note: EventHandler<Highlight>,
    on_delete: impl FnMut(MouseEvent) + 'static,
) -> Element {
    rsx! {
        div { class: "rd-hl-actions",
            button {
                class: "rd-act sm",
                r#type: "button",
                "data-testid": "highlight-note",
                onclick: {
                    let h = highlight.clone();
                    move |_| on_edit_note.call(h.clone())
                },
                "{note_label}"
            }
            button {
                class: "rd-act sm",
                r#type: "button",
                "data-testid": "highlight-quote",
                onclick: {
                    let h = highlight.clone();
                    move |_| on_quote.call(h.clone())
                },
                "Quote"
            }
            button {
                class: "rd-act sm",
                r#type: "button",
                disabled: !has_text,
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

#[component]
pub(super) fn HighlightRow(
    highlight: Highlight,
    highlights: Signal<Vec<Highlight>>,
    on_quote: EventHandler<Highlight>,
    on_edit_note: EventHandler<Highlight>,
) -> Element {
    let server_url = crate::contexts::use_server_url();
    let color = highlight.color.as_str();
    let quote = highlight
        .text
        .clone()
        .unwrap_or_else(|| "(highlighted passage)".to_string());
    let note = highlight.note.clone();
    let cfi = highlight.epub_cfi_range.clone();
    let copy_src = highlight.text.clone().unwrap_or_default();
    // Legacy highlights created before the text column have nothing to copy;
    // disable the action rather than offer a silent no-op.
    let has_text = highlight.text.is_some();
    // The note button both adds a first note and edits an existing one; its
    // label reflects which so the affordance reads correctly either way.
    let note_label = if highlight.note.is_some() {
        "Edit note"
    } else {
        "Add note"
    };
    let id = highlight.id;
    let on_delete = build_delete_handler(server_url.clone(), highlights, id, cfi);

    let cur_color = highlight.color;
    let recolor_cfi = highlight.epub_cfi_range.clone();
    let nav_cfi = highlight.epub_cfi_range.clone();

    rsx! {
        div { class: "rd-hl-row", "data-testid": "reader-highlight-row",
            style: "border-left-color: var(--hl-{color});",
            button {
                class: "rd-hl-quote",
                r#type: "button",
                // Anchorless rows have nowhere to jump; the quote still shows.
                disabled: nav_cfi.is_none(),
                onclick: move |_| {
                    if let Some(cfi) = &nav_cfi {
                        navigate_to(cfi);
                    }
                },
                "\u{201c}{quote}\u{201d}"
            }
            if let Some(n) = note {
                div { class: "rd-hl-note", "{n}" }
            }
            {hl_swatch_strip(cur_color, recolor_cfi, server_url, highlights, id)}
            {hl_actions_row(&highlight, note_label, has_text, copy_src, on_quote, on_edit_note, on_delete)}
        }
    }
}
