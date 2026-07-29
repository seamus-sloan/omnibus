//! Combined phone annotations sheet: every highlight, note, and bookmark for
//! the open book in one bottom sheet with kind + color filter chips. The phone
//! toolbar's single "Annotations" tool opens this; desktop keeps its separate
//! highlights and bookmarks drawers. Renders on every target (rule 07); only
//! the phone breakpoint surfaces the button that opens it.

use dioxus::prelude::*;

#[cfg(any(feature = "web", feature = "mobile"))]
use omnibus_shared::CreateBookmark;
use omnibus_shared::{Bookmark, Highlight, HighlightColor};

use super::highlights_drawer::{HighlightRow, PALETTE};

/// Which annotation kind/color the sheet is showing.
#[derive(Clone, Copy, PartialEq)]
enum AnnFilter {
    All,
    Bookmarks,
    Color(HighlightColor),
}

/// Header count line: "4 highlights · 1 bookmark". Singular/plural per count.
fn format_annotation_counts(highlights: usize, bookmarks: usize) -> String {
    let h = if highlights == 1 {
        "highlight"
    } else {
        "highlights"
    };
    let b = if bookmarks == 1 {
        "bookmark"
    } else {
        "bookmarks"
    };
    format!("{highlights} {h} \u{b7} {bookmarks} {b}")
}

/// Props for [`AnnotationsSheet`]: the open book's identity/position plus the
/// highlight list and the callbacks it forwards to its rows.
#[derive(Props, Clone, PartialEq)]
pub(super) struct AnnotationsSheetProps {
    /// UUID of the open book, used to load/create bookmarks.
    pub uuid: String,
    /// Current reader CFI, used as the position for a new bookmark.
    pub current_cfi: String,
    /// Current chapter label, used as the default title for a new bookmark.
    pub current_label: String,
    /// The open book's highlights, filterable by kind/color.
    pub highlights: Signal<Vec<Highlight>>,
    /// Fired with a highlight when its "Quote" action is tapped.
    pub on_quote: EventHandler<Highlight>,
    /// Fired with a highlight when its "Add/Edit note" action is tapped.
    pub on_edit_note: EventHandler<Highlight>,
    /// Fired with a CFI when a bookmark row is tapped.
    pub on_navigate: EventHandler<String>,
    /// Fired when the scrim or close button dismisses the sheet.
    pub on_close: EventHandler<()>,
}

/// Drawer head: title + count, "+ Bookmark", and close button.
fn render_annotations_header(
    counts: String,
    can_add: bool,
    on_add: impl FnMut(Event<MouseData>) + 'static,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "rd-drawer-head",
            h4 { class: "rd-drawer-title",
                "Annotations "
                span { class: "rd-drawer-count", "{counts}" }
            }
            div { class: "rd-drawer-head-actions",
                button {
                    class: "btn sm",
                    r#type: "button",
                    "data-testid": "reader-annotations-add",
                    disabled: !can_add,
                    onclick: on_add,
                    "+ Bookmark"
                }
                button {
                    class: "rd-x",
                    r#type: "button",
                    "aria-label": "Close",
                    onclick: move |_| on_close.call(()),
                    "\u{00d7}"
                }
            }
        }
    }
}

/// Kind/color filter chip row: All, Bookmarks, then one chip per highlight color.
fn render_annotations_filter_row(active: AnnFilter, filter: Signal<AnnFilter>) -> Element {
    let mut filter = filter;
    rsx! {
        div { class: "rd-filter-row",
            button {
                class: if active == AnnFilter::All { "rd-chip on" } else { "rd-chip" },
                r#type: "button",
                onclick: move |_| filter.set(AnnFilter::All),
                "All"
            }
            button {
                class: if active == AnnFilter::Bookmarks { "rd-chip on" } else { "rd-chip" },
                r#type: "button",
                "data-testid": "annotations-filter-bookmarks",
                onclick: move |_| filter.set(AnnFilter::Bookmarks),
                "Bookmarks"
            }
            span { class: "rd-filter-div" }
            for (color, name) in PALETTE {
                button {
                    key: "{name}",
                    class: if active == AnnFilter::Color(color) { "rd-chip on" } else { "rd-chip" },
                    r#type: "button",
                    "aria-label": "Filter {name}",
                    onclick: move |_| filter.set(AnnFilter::Color(color)),
                    span { class: "rd-chip-dot", "data-color": "{name}" }
                }
            }
        }
    }
}

/// Drawer body: empty state, or the filtered bookmark + highlight rows.
#[allow(clippy::too_many_arguments)]
fn render_annotations_body(
    empty: bool,
    show_bookmarks: bool,
    marks: Vec<Bookmark>,
    shown_highlights: Vec<Highlight>,
    bookmarks: Signal<Vec<Bookmark>>,
    highlights: Signal<Vec<Highlight>>,
    on_navigate: EventHandler<String>,
    on_quote: EventHandler<Highlight>,
    on_edit_note: EventHandler<Highlight>,
) -> Element {
    rsx! {
        div { class: "rd-drawer-body",
            if empty {
                div { class: "rd-drawer-empty",
                    "Nothing here yet. Highlight a passage or tap + Bookmark to save your place."
                }
            } else {
                if show_bookmarks {
                    for mark in marks.iter() {
                        AnnotationBookmarkRow {
                            key: "bm-{mark.id}",
                            bookmark: mark.clone(),
                            on_navigate,
                            list: bookmarks,
                        }
                    }
                }
                for h in shown_highlights.iter() {
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

/// The combined annotations bottom sheet.
#[component]
pub(super) fn AnnotationsSheet(props: AnnotationsSheetProps) -> Element {
    let AnnotationsSheetProps {
        uuid,
        current_cfi,
        current_label,
        highlights,
        on_quote,
        on_edit_note,
        on_navigate,
        on_close,
    } = props;
    let filter = use_signal(|| AnnFilter::All);
    let bookmarks = use_signal(Vec::<Bookmark>::new);
    let server_url = crate::contexts::use_server_url();

    // Load existing bookmarks after mount (SSR renders empty for hydration
    // parity; the load effect is declared unconditionally).
    let uuid_load = uuid.clone();
    let server_load = server_url.clone();
    use_effect(move || {
        let _uuid = uuid_load.clone();
        let _server = server_load.clone();
        let mut _list = bookmarks;
        #[cfg(any(feature = "web", feature = "mobile"))]
        spawn(async move {
            if let Ok(items) = crate::data::list_bookmarks(&_server, &_uuid).await {
                _list.set(items);
            }
        });
    });

    let can_add = !current_cfi.is_empty();
    let add = {
        let server_url = server_url.clone();
        move |_| {
            let cfi = current_cfi.clone();
            let label = current_label.clone();
            let uuid = uuid.clone();
            let server_url = server_url.clone();
            let mut list = bookmarks;
            #[cfg(any(feature = "web", feature = "mobile"))]
            spawn(async move {
                let input = CreateBookmark {
                    client_id: None,
                    book_uuid: uuid,
                    position: cfi,
                    title: if label.is_empty() { None } else { Some(label) },
                };
                if let Ok(b) = crate::data::create_bookmark(&server_url, input).await {
                    list.write().push(b);
                }
            });
            #[cfg(not(any(feature = "web", feature = "mobile")))]
            let _ = (cfi, label, uuid, server_url, &mut list);
        }
    };

    let all_highlights = highlights.read().clone();
    let marks = bookmarks.read().clone();
    let active = filter();
    let counts = format_annotation_counts(all_highlights.len(), marks.len());

    let show_bookmarks = matches!(active, AnnFilter::All | AnnFilter::Bookmarks);
    let shown_highlights: Vec<Highlight> = match active {
        AnnFilter::Bookmarks => Vec::new(),
        AnnFilter::All => all_highlights.clone(),
        AnnFilter::Color(c) => all_highlights
            .iter()
            .filter(|h| h.color == c)
            .cloned()
            .collect(),
    };
    let empty = (!show_bookmarks || marks.is_empty()) && shown_highlights.is_empty();

    rsx! {
        div { class: "rd-scrim", onclick: move |_| on_close.call(()) }
        div { class: "rd-drawer", "data-testid": "reader-annotations-drawer",
            div { class: "rd-grabber" }
            {render_annotations_header(counts, can_add, add, on_close)}
            {render_annotations_filter_row(active, filter)}
            {
                render_annotations_body(
                    empty,
                    show_bookmarks,
                    marks,
                    shown_highlights,
                    bookmarks,
                    highlights,
                    on_navigate,
                    on_quote,
                    on_edit_note,
                )
            }
        }
    }
}

/// One bookmark row in the combined sheet: chapter label + delete.
#[component]
fn AnnotationBookmarkRow(
    bookmark: Bookmark,
    on_navigate: EventHandler<String>,
    list: Signal<Vec<Bookmark>>,
) -> Element {
    let id = bookmark.id;
    let server_url = crate::contexts::use_server_url();
    let cfi = bookmark.position.clone();
    let label = bookmark
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Bookmark".to_string());

    let on_delete = move |_| {
        let mut list = list;
        let server_url = server_url.clone();
        #[cfg(any(feature = "web", feature = "mobile"))]
        spawn(async move {
            if crate::data::delete_bookmark(&server_url, id).await.is_ok() {
                list.write().retain(|b| b.id != id);
            }
        });
        #[cfg(not(any(feature = "web", feature = "mobile")))]
        let _ = (&mut list, id, server_url);
    };

    rsx! {
        div { class: "rd-bm-row rd-ann-bm", "data-testid": "annotations-bookmark-row",
            button {
                class: "rd-bm-main",
                r#type: "button",
                onclick: move |_| on_navigate.call(cfi.clone()),
                span { class: "rd-bm-flag",
                    svg {
                        width: "16", height: "16", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M7 4h10v16l-5-3.6L7 20V4z" }
                    }
                }
                span { class: "rd-bm-label", "{label}" }
            }
            button {
                class: "rd-act sm",
                r#type: "button",
                "aria-label": "Delete bookmark",
                onclick: on_delete,
                "\u{00d7}"
            }
        }
    }
}

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod tests {
    use super::*;

    #[test]
    fn format_annotation_counts_pluralizes_per_count() {
        assert_eq!(
            format_annotation_counts(4, 1),
            "4 highlights \u{b7} 1 bookmark"
        );
        assert_eq!(
            format_annotation_counts(1, 0),
            "1 highlight \u{b7} 0 bookmarks"
        );
    }
}
