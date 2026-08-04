//! Bookmarks drawer for the listen page.
//!
//! Right-side full-height panel listing saved bookmarks. Each row seeks on
//! click, shows the derived chapter label + timestamp, an inline note editor,
//! and a delete action. The freshest bookmark gets an accent ring.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{Bookmark, ChapterInfo};

use super::bookmarks::{bookmark_position_seconds, chapter_label_at, BookmarksController};
use super::helpers::format_hms;
use super::panel_shell::ListenDrawerShell;

#[component]
pub(super) fn BookmarksDrawer(
    controller: BookmarksController,
    chapters: Vec<ChapterInfo>,
    on_seek: EventHandler<f64>,
    on_add: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    // Hold the read guard and iterate it directly — no need to clone the whole
    // list on every render (rows clone individual bookmarks as they need them).
    let marks = controller.list.read();
    let fresh = (controller.fresh_id)();
    let count = marks.len();

    rsx! {
        ListenDrawerShell {
            testid: "bookmarks-drawer",
            on_close,
            head: rsx! {
                div {
                    div { class: "lp-panel-kicker", "Bookmarks \u{00b7} {count}" }
                    div { class: "lp-panel-title", "Your marks" }
                }
            },
            actions: rsx! {
                button {
                    class: "btn sm",
                    r#type: "button",
                    "data-testid": "bookmark-add",
                    onclick: move |_| on_add.call(()),
                    "+ Bookmark"
                }
            },
            if marks.is_empty() {
                div { class: "lp-drawer-empty",
                    p { class: "lp-drawer-empty-title", "No bookmarks yet" }
                    p { class: "lp-drawer-empty-detail",
                        "Tap the Bookmark button while listening to save "
                        "your place. Add a note to any mark below."
                    }
                }
            } else {
                for mark in marks.iter() {
                    BookmarkRow {
                        key: "{mark.id}",
                        bookmark: mark.clone(),
                        chapters: chapters.clone(),
                        is_fresh: fresh == Some(mark.id),
                        on_seek,
                        controller,
                    }
                }
            }
        }
    }
}

#[component]
fn BookmarkRow(
    bookmark: Bookmark,
    chapters: Vec<ChapterInfo>,
    is_fresh: bool,
    on_seek: EventHandler<f64>,
    controller: BookmarksController,
) -> Element {
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(|| bookmark.title.clone().unwrap_or_default());

    let pos = bookmark_position_seconds(&bookmark);
    let label = chapter_label_at(pos, &chapters);
    let time_label = format_hms(pos);
    let row_class = if is_fresh {
        "lp-bm-row fresh"
    } else {
        "lp-bm-row"
    };
    let note = bookmark.title.clone();
    let id = bookmark.id;

    // `Fn` (not `FnMut`) so it can be copied into both the blur and keydown
    // handlers: it mutates a local copy of the `editing` signal, not a
    // captured-by-mut binding.
    let do_commit = move || {
        let mut editing = editing;
        let text = draft.peek().trim().to_string();
        let next = if text.is_empty() { None } else { Some(text) };
        controller.rename(id, next);
        editing.set(false);
    };

    rsx! {
        div { class: "{row_class}", "data-testid": "bookmark-row",
            button {
                class: "lp-bm-main",
                r#type: "button",
                onclick: move |_| on_seek.call(pos),
                span { class: "lp-bm-flag", "\u{2691}" }
                span { class: "lp-bm-label", "{label}" }
                span { class: "lp-bm-time", "{time_label}" }
            }
            div { class: "lp-bm-note",
                if editing() {
                    input {
                        class: "lp-bm-note-input",
                        r#type: "text",
                        placeholder: "Add a note\u{2026}",
                        autofocus: true,
                        value: "{draft}",
                        oninput: move |e| draft.set(e.value()),
                        onblur: move |_| do_commit(),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                do_commit();
                            }
                        },
                    }
                } else {
                    button {
                        class: "lp-bm-note-text",
                        r#type: "button",
                        onclick: move |_| editing.set(true),
                        if let Some(n) = note.clone() {
                            "{n}"
                        } else {
                            span { class: "lp-bm-note-empty", "Add a note\u{2026}" }
                        }
                    }
                }
            }
            button {
                class: "lp-bm-del",
                r#type: "button",
                "aria-label": "Delete bookmark",
                title: "Delete bookmark",
                onclick: move |_| controller.remove(id),
                "\u{00d7}"
            }
        }
    }
}
