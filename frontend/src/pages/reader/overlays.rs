//! Reader overlays: the live-selection `ReaderSelectionPopover` and the
//! toggleable `ReaderOverlays` (contents / highlights / search / bookmarks
//! drawers plus quote panel and note composer). Each renders only when its
//! backing signal is set. Compiles on every target; the JS glue bridge runs on
//! web + mobile (SSR no-ops).

use dioxus::prelude::*;

use omnibus_shared::{Highlight, HighlightColor};

use super::annotations_sheet::AnnotationsSheet;
use super::highlights::{spawn_create_highlight, HighlightTargets, NewHighlight, PostCreate};
use super::highlights_drawer::HighlightsDrawer;
use super::note_composer::NoteComposer;
use super::quote_panel::QuotePanel;
use super::reader_bookmarks::ReaderBookmarksDrawer;
use super::search_panel::SearchPanel;
use super::selection::{SelectionActions, SelectionAnchor, SelectionData, SelectionPopover};
use super::toc_drawer::TocDrawer;
use super::ReaderPanelSignals;

/// Selection popover: highlight / note / quote / copy / share actions for the
/// current text selection. Renders nothing when there is no selection.
#[component]
pub(super) fn ReaderSelectionPopover(
    uuid: String,
    selection: Signal<Option<SelectionData>>,
    highlights: Signal<Vec<Highlight>>,
    note_target: Signal<Option<Highlight>>,
    quote_target: Signal<Option<Highlight>>,
) -> Element {
    let server_url = crate::contexts::use_server_url();
    let targets = HighlightTargets {
        highlights,
        note_target,
        quote_target,
    };
    let Some(sel) = selection.read().as_ref().cloned() else {
        return rsx! {};
    };
    rsx! {
        SelectionPopover {
            anchor: SelectionAnchor {
                sel_rect_x: sel.rect.x,
                sel_rect_y: sel.rect.y,
                sel_rect_width: sel.rect.width,
                sel_cfi: sel.cfi_range.clone(),
                sel_text: sel.text.clone(),
            },
            actions: SelectionActions {
                on_dismiss: EventHandler::new(move |_| {
                    let mut selection = selection;
                    selection.set(None);
                }),
                on_highlight: EventHandler::new({
                    let uuid = uuid.clone();
                    let server_url = server_url.clone();
                    move |(cfi, color, text): (String, HighlightColor, String)| {
                        let mut selection = selection;
                        selection.set(None);
                        spawn_create_highlight(
                            server_url.clone(),
                            uuid.clone(),
                            NewHighlight { cfi, color, text, post: PostCreate::None },
                            targets,
                        );
                    }
                }),
                on_note: EventHandler::new({
                    let uuid = uuid.clone();
                    let server_url = server_url.clone();
                    move |(cfi, text): (String, String)| {
                        let mut selection = selection;
                        selection.set(None);
                        spawn_create_highlight(
                            server_url.clone(),
                            uuid.clone(),
                            NewHighlight { cfi, color: HighlightColor::Amber, text, post: PostCreate::Note },
                            targets,
                        );
                    }
                }),
                on_quote: EventHandler::new({
                    let uuid = uuid.clone();
                    let server_url = server_url.clone();
                    move |(cfi, text): (String, String)| {
                        let mut selection = selection;
                        selection.set(None);
                        spawn_create_highlight(
                            server_url.clone(),
                            uuid.clone(),
                            NewHighlight { cfi, color: HighlightColor::Amber, text, post: PostCreate::Quote },
                            targets,
                        );
                    }
                }),
                on_copy: EventHandler::new(move |text: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("copyText", &text);
                    let _ = &text;
                    let mut selection = selection;
                    selection.set(None);
                }),
                on_share: EventHandler::new(move |text: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("shareText", &text);
                    let _ = &text;
                    let mut selection = selection;
                    selection.set(None);
                }),
            },
        }
    }
}

/// Book identity + display strings the overlay drawers render.
#[derive(Clone, PartialEq)]
pub(super) struct OverlayMeta {
    pub uuid: String,
    pub book_title: String,
    pub book_author: String,
    pub book_accent: String,
    pub chapter_title: String,
    pub current_cfi: String,
    /// Contents-drawer progress line ("184 / 272 · 68%").
    pub contents_progress: String,
}

/// The toggleable reader overlays: contents, highlights, search, bookmarks
/// drawers plus the quote panel and note composer. Each renders only when its
/// backing signal is set.
#[component]
pub(super) fn ReaderOverlays(meta: OverlayMeta, panels: ReaderPanelSignals) -> Element {
    let OverlayMeta {
        uuid,
        book_title,
        book_author,
        book_accent,
        chapter_title,
        current_cfi,
        contents_progress,
    } = meta;
    let ReaderPanelSignals {
        highlights,
        toc,
        show_toc,
        show_highlights,
        note_target,
        quote_target,
        show_search,
        search_results,
        show_bookmarks,
        show_annotations,
        ..
    } = panels;

    rsx! {
        if show_toc() {
            TocDrawer {
                entries: toc.read().clone(),
                current_title: chapter_title.clone(),
                progress_label: contents_progress.clone(),
                on_navigate: move |href: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("display", &href);
                    let _ = &href;
                    let mut show_toc = show_toc;
                    show_toc.set(false);
                },
                on_close: move |_| {
                    let mut show_toc = show_toc;
                    show_toc.set(false);
                },
            }
        }

        if show_highlights() {
            HighlightsDrawer {
                highlights,
                on_quote: move |h: Highlight| {
                    let mut quote_target = quote_target;
                    let mut show_highlights = show_highlights;
                    quote_target.set(Some(h));
                    show_highlights.set(false);
                },
                on_edit_note: move |h: Highlight| {
                    let mut note_target = note_target;
                    let mut show_highlights = show_highlights;
                    note_target.set(Some(h));
                    show_highlights.set(false);
                },
                on_close: move |_| {
                    let mut show_highlights = show_highlights;
                    show_highlights.set(false);
                },
            }
        }

        if show_search() {
            SearchPanel {
                results: search_results,
                on_query: move |q: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("search", &q);
                    let _ = &q;
                },
                on_navigate: move |cfi: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("display", &cfi);
                    let _ = &cfi;
                    let mut show_search = show_search;
                    show_search.set(false);
                },
                on_close: move |_| {
                    let mut show_search = show_search;
                    show_search.set(false);
                },
            }
        }

        if show_bookmarks() {
            ReaderBookmarksDrawer {
                uuid: uuid.clone(),
                current_cfi: current_cfi.clone(),
                current_label: chapter_title.clone(),
                on_navigate: move |cfi: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("display", &cfi);
                    let _ = &cfi;
                    let mut show_bookmarks = show_bookmarks;
                    show_bookmarks.set(false);
                },
                on_close: move |_| {
                    let mut show_bookmarks = show_bookmarks;
                    show_bookmarks.set(false);
                },
            }
        }

        if show_annotations() {
            AnnotationsSheet {
                uuid: uuid.clone(),
                current_cfi: current_cfi.clone(),
                current_label: chapter_title.clone(),
                highlights,
                on_quote: move |h: Highlight| {
                    let mut quote_target = quote_target;
                    let mut show_annotations = show_annotations;
                    quote_target.set(Some(h));
                    show_annotations.set(false);
                },
                on_edit_note: move |h: Highlight| {
                    let mut note_target = note_target;
                    let mut show_annotations = show_annotations;
                    note_target.set(Some(h));
                    show_annotations.set(false);
                },
                on_navigate: move |cfi: String| {
                    #[cfg(any(feature = "web", feature = "mobile"))]
                    super::reader_call_json("display", &cfi);
                    let _ = &cfi;
                    let mut show_annotations = show_annotations;
                    show_annotations.set(false);
                },
                on_close: move |_| {
                    let mut show_annotations = show_annotations;
                    show_annotations.set(false);
                },
            }
        }

        if let Some(h) = quote_target.read().clone() {
            QuotePanel {
                quote_text: h.text.clone().unwrap_or_default(),
                author: book_author.clone(),
                subtitle: book_title.clone(),
                accent: book_accent.clone(),
                on_close: move |_| {
                    let mut quote_target = quote_target;
                    quote_target.set(None);
                },
            }
        }

        if let Some(h) = note_target.read().clone() {
            NoteComposer {
                highlight: h,
                on_saved: move |(id, note): (i64, Option<String>)| {
                    let mut highlights = highlights;
                    let idx = highlights.read().iter().position(|x| x.id == id);
                    if let Some(i) = idx {
                        highlights.write()[i].note = note;
                    }
                },
                on_close: move |_| {
                    let mut note_target = note_target;
                    note_target.set(None);
                },
            }
        }
    }
}
