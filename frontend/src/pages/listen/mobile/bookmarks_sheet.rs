//! Bookmarks for the mobile player: the loaded list + fresh-mark + toast
//! controller (the mobile mirror of the web `bookmarks::BookmarksController`)
//! and the bottom sheet that renders it. Positions are audiobook seconds
//! stored as the bookmark's opaque `position` token.

use dioxus::prelude::*;
use omnibus_shared::{Bookmark, ChapterInfo, CreateBookmark};

use super::sheets::MSheet;
use super::view::{chapter_index_for_elapsed, format_hms};

/// Transient "Bookmark saved" toast payload: `H:MM:SS · Ch. N`.
pub(super) type BookmarkToast = String;

/// Bookmark-state handle returned by [`use_mobile_bookmarks`]. Cheap to copy.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct MobileBookmarks {
    pub list: Signal<Vec<Bookmark>>,
    pub fresh_id: Signal<Option<i64>>,
    pub toast: Signal<Option<BookmarkToast>>,
    uuid: Signal<String>,
    server_url: Signal<String>,
}

impl MobileBookmarks {
    /// Create a bookmark at `position_secs`, prepend it to the list, flag it
    /// fresh, and raise an auto-dismissing toast.
    pub fn create(&self, position_secs: f64, chapters: &[ChapterInfo]) {
        let uuid = self.uuid.peek().clone();
        let server_url = self.server_url.peek().clone();
        let mut list = self.list;
        let mut fresh_id = self.fresh_id;
        let mut toast = self.toast;
        let label = toast_label(position_secs, chapters);
        let input = CreateBookmark {
            book_uuid: uuid,
            position: position_secs.to_string(),
            title: None,
        };
        spawn(async move {
            if let Ok(b) = crate::data::create_bookmark(&server_url, input).await {
                let id = b.id;
                fresh_id.set(Some(id));
                list.write().insert(0, b);
                toast.set(Some(label));
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                // Leave a newer toast alone; only clear our own.
                if *fresh_id.peek() == Some(id) {
                    toast.set(None);
                }
            }
        });
    }

    /// Persist a new note on a bookmark and update the local row.
    pub fn rename(&self, id: i64, title: Option<String>) {
        let server_url = self.server_url.peek().clone();
        let mut list = self.list;
        let title_for_local = title.clone();
        spawn(async move {
            if crate::data::update_bookmark(&server_url, id, title)
                .await
                .is_ok()
            {
                if let Some(row) = list.write().iter_mut().find(|b| b.id == id) {
                    row.title = title_for_local;
                }
            }
        });
    }

    /// Delete a bookmark and drop it from the local list.
    pub fn remove(&self, id: i64) {
        let server_url = self.server_url.peek().clone();
        let mut list = self.list;
        spawn(async move {
            if crate::data::delete_bookmark(&server_url, id).await.is_ok() {
                list.write().retain(|b| b.id != id);
            }
        });
    }
}

/// Toast body for a mark at `position_secs`.
fn toast_label(position_secs: f64, chapters: &[ChapterInfo]) -> String {
    let time = format_hms(position_secs);
    if chapters.is_empty() {
        return time;
    }
    let idx = chapter_index_for_elapsed(chapters, position_secs);
    format!("{time} \u{00b7} Ch. {}", idx + 1)
}

/// Parse a bookmark's opaque position token as audiobook seconds.
fn bookmark_position_seconds(b: &Bookmark) -> f64 {
    b.position.parse::<f64>().unwrap_or(0.0)
}

/// Row title, e.g. "Ch. 14 · A Sea of Glass and Fire".
fn row_label(position_secs: f64, chapters: &[ChapterInfo]) -> String {
    if chapters.is_empty() {
        return "Bookmark".to_string();
    }
    let idx = chapter_index_for_elapsed(chapters, position_secs);
    match chapters.get(idx) {
        Some(c) if !c.title.is_empty() => format!("Ch. {} \u{00b7} {}", idx + 1, c.title),
        Some(_) => format!("Chapter {}", idx + 1),
        None => "Bookmark".to_string(),
    }
}

/// Install the bookmark signals and load the existing list after mount.
/// The unified table can also hold reader (CFI) bookmarks for a dual-format
/// book; the listen sheet is audio-only, so non-numeric positions are dropped.
pub(super) fn use_mobile_bookmarks(uuid: String, server_url: String) -> MobileBookmarks {
    let list = use_signal(Vec::<Bookmark>::new);
    let fresh_id = use_signal(|| None::<i64>);
    let toast = use_signal(|| None::<BookmarkToast>);
    let uuid_sig = use_signal(|| uuid);
    let server_url_sig = use_signal(|| server_url);

    use_effect(move || {
        let uuid = uuid_sig.peek().clone();
        let server_url = server_url_sig.peek().clone();
        let mut list = list;
        spawn(async move {
            if let Ok(items) = crate::data::list_bookmarks(&server_url, &uuid).await {
                list.set(
                    items
                        .into_iter()
                        .filter(|b| b.position.parse::<f64>().is_ok())
                        .collect(),
                );
            }
        });
    });

    MobileBookmarks {
        list,
        fresh_id,
        toast,
        uuid: uuid_sig,
        server_url: server_url_sig,
    }
}

/// Bottom-sheet bookmark list. The fresh mark is accent-highlighted with an
/// inline "Add a note…" editor; every row seeks on tap and carries a delete.
#[component]
pub(super) fn BookmarksSheet(
    bookmarks: MobileBookmarks,
    chapters: Vec<ChapterInfo>,
    on_seek: EventHandler<f64>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let editing = use_signal(|| None::<i64>);
    let draft = use_signal(String::new);
    let list = bookmarks.list.read().clone();
    let fresh = (bookmarks.fresh_id)();
    let count = list.len();
    let count_label = if count == 1 {
        "1 mark".to_string()
    } else {
        format!("{count} marks")
    };
    rsx! {
        MSheet {
            title: "Bookmarks",
            testid: "mobile-bookmarks-sheet",
            meta: rsx! {
                span { class: "mono m-sheet-count", "{count_label}" }
            },
            on_close,
            div { class: "m-sheet-list",
                if list.is_empty() {
                    p { class: "subtitle m-bm-empty", "No bookmarks yet \u{2014} tap Bookmark to mark this spot." }
                }
                for b in list.into_iter() {
                    {bookmark_row(&b, BookmarkRowCtx { chapters: &chapters, fresh, bookmarks, on_seek, editing, draft })}
                }
            }
        }
    }
}

/// Sheet-level rendering context shared by every [`bookmark_row`]: the
/// chapter map plus the controller and edit-state signals each row wires to.
#[derive(Clone, Copy)]
struct BookmarkRowCtx<'a> {
    chapters: &'a [ChapterInfo],
    fresh: Option<i64>,
    bookmarks: MobileBookmarks,
    on_seek: EventHandler<f64>,
    editing: Signal<Option<i64>>,
    draft: Signal<String>,
}

/// One bookmark row (plain fn — no hooks).
fn bookmark_row(b: &Bookmark, ctx: BookmarkRowCtx<'_>) -> Element {
    let BookmarkRowCtx {
        chapters,
        fresh,
        bookmarks,
        on_seek,
        editing,
        draft,
    } = ctx;
    let id = b.id;
    let pos = bookmark_position_seconds(b);
    let is_fresh = fresh == Some(id);
    let label = row_label(pos, chapters);
    let note = b.title.clone().unwrap_or_default();
    let is_editing = editing() == Some(id);
    let seek = on_seek;
    let mut editing_sig = editing;
    let mut draft_sig = draft;
    let note_seed = note.clone();
    let save_draft = draft_sig;
    rsx! {
        div {
            key: "{id}",
            class: if is_fresh { "m-bm-row fresh" } else { "m-bm-row" },
            span { class: "m-bm-flag", "\u{2691}" }
            div { class: "m-bm-body",
                button {
                    r#type: "button", class: "m-bm-jump",
                    onclick: move |_| seek.call(pos),
                    div { class: "m-bm-title", span { class: "m-em", "{label}" } }
                }
                if is_editing {
                    div { class: "m-bm-edit",
                        input {
                            r#type: "text",
                            class: "m-bm-note-input",
                            placeholder: "Add a note\u{2026}",
                            value: "{draft_sig}",
                            oninput: move |e| draft_sig.set(e.value()),
                        }
                        button {
                            r#type: "button", class: "btn sm",
                            onclick: move |_| {
                                let text = save_draft.peek().trim().to_string();
                                bookmarks.rename(id, (!text.is_empty()).then_some(text));
                                editing_sig.set(None);
                            },
                            "Save"
                        }
                    }
                } else if !note.is_empty() {
                    button {
                        r#type: "button", class: "m-bm-note",
                        onclick: move |_| { draft_sig.set(note_seed.clone()); editing_sig.set(Some(id)); },
                        "{note}"
                    }
                } else {
                    button {
                        r#type: "button", class: "m-bm-note placeholder",
                        onclick: move |_| { draft_sig.set(String::new()); editing_sig.set(Some(id)); },
                        "Add a note\u{2026}"
                    }
                }
            }
            span { class: "mono m-bm-time", "{format_hms(pos)}" }
            button {
                r#type: "button", class: "m-bm-del", "aria-label": "Delete bookmark",
                onclick: move |_| bookmarks.remove(id),
                "\u{00d7}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(start: f64, title: &str) -> ChapterInfo {
        ChapterInfo {
            ordinal: 1,
            title: title.into(),
            start_seconds: start,
            duration_seconds: 300.0,
        }
    }

    fn bm(position: &str) -> Bookmark {
        Bookmark {
            id: 1,
            book_uuid: "u".into(),
            position: position.into(),
            title: None,
            created_at: 0,
        }
    }

    #[test]
    fn bookmark_position_seconds_parses_decimal_and_defaults_on_garbage() {
        assert!((bookmark_position_seconds(&bm("123.5")) - 123.5).abs() < f64::EPSILON);
        assert_eq!(bookmark_position_seconds(&bm("epubcfi(/6/4)")), 0.0);
    }

    #[test]
    fn row_label_formats_titled_chapter_and_falls_back() {
        let chs = vec![ch(0.0, "Intro"), ch(300.0, "Part One")];
        assert_eq!(row_label(420.0, &chs), "Ch. 2 \u{00b7} Part One");
        assert_eq!(row_label(10.0, &[]), "Bookmark");
        assert_eq!(row_label(10.0, &[ch(0.0, "")]), "Chapter 1");
    }

    #[test]
    fn toast_label_includes_time_and_chapter() {
        let chs = vec![ch(0.0, "Intro"), ch(300.0, "Part One")];
        assert_eq!(toast_label(420.0, &chs), "7:00 \u{00b7} Ch. 2");
        assert_eq!(toast_label(420.0, &[]), "7:00");
    }
}
