//! Bookmark state for the listen page.
//!
//! Owns the loaded list, the freshly-created id (for the accent ring), and
//! the confirmation toast. Pure helpers derive the chapter label and toast
//! text from a bookmark's stored position. `ready_player` drives creation;
//! `bookmarks_drawer` renders the list.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{Bookmark, ChapterInfo};

use super::helpers::format_hms;
use super::ready_player::chapter_index_for_elapsed;

/// Transient "Bookmark saved" toast payload.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct BookmarkToast {
    pub time_label: String,
    pub chapter_label: String,
}

/// Parse a bookmark's opaque position token as audiobook seconds.
pub(super) fn bookmark_position_seconds(b: &Bookmark) -> f64 {
    b.position.parse::<f64>().unwrap_or(0.0)
}

/// Human chapter label for a position, e.g. "Ch. 14 · A Sea of Glass and Fire".
/// Falls back to a bare ordinal, then a generic label when no chapters exist.
pub(super) fn chapter_label_at(position_secs: f64, chapters: &[ChapterInfo]) -> String {
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

/// Bookmark-state handle returned by [`use_bookmarks`]. Cheap to copy.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct BookmarksController {
    pub list: Signal<Vec<Bookmark>>,
    pub fresh_id: Signal<Option<i64>>,
    pub toast: Signal<Option<BookmarkToast>>,
    uuid: Signal<String>,
}

impl BookmarksController {
    /// Create a bookmark at `position_secs`, push it onto the list, flag it as
    /// fresh, and raise an auto-dismissing toast. Web-only side effects.
    pub fn create(&self, position_secs: f64, chapters: &[ChapterInfo]) {
        let _chapter_label = chapter_label_at(position_secs, chapters);
        let _time_label = format_hms(position_secs);
        #[cfg(feature = "web")]
        {
            let uuid = self.uuid.peek().clone();
            let mut list = self.list;
            let mut fresh_id = self.fresh_id;
            let mut toast = self.toast;
            let input = omnibus_shared::CreateBookmark {
                book_uuid: uuid,
                position: position_secs.to_string(),
                title: None,
            };
            spawn(async move {
                if let Ok(b) = crate::data::create_bookmark("", input).await {
                    fresh_id.set(Some(b.id));
                    list.write().push(b);
                    toast.set(Some(BookmarkToast {
                        time_label: _time_label,
                        chapter_label: _chapter_label,
                    }));
                    gloo_timers::future::sleep(std::time::Duration::from_secs(3)).await;
                    toast.set(None);
                }
            });
        }
    }

    /// Persist a new title/note on a bookmark and update the local row.
    pub fn rename(&self, id: i64, title: Option<String>) {
        #[cfg(feature = "web")]
        {
            let mut list = self.list;
            let title_for_local = title.clone();
            spawn(async move {
                if crate::data::update_bookmark("", id, title).await.is_ok() {
                    if let Some(row) = list.write().iter_mut().find(|b| b.id == id) {
                        row.title = title_for_local;
                    }
                }
            });
        }
        #[cfg(not(feature = "web"))]
        let _ = (id, title);
    }

    /// Delete a bookmark and drop it from the local list.
    pub fn remove(&self, id: i64) {
        #[cfg(feature = "web")]
        {
            let mut list = self.list;
            spawn(async move {
                if crate::data::delete_bookmark("", id).await.is_ok() {
                    list.write().retain(|b| b.id != id);
                }
            });
        }
        #[cfg(not(feature = "web"))]
        let _ = id;
    }
}

/// Install the bookmark signals and load the existing list after mount.
/// The load effect is declared unconditionally (hook-order parity); only the
/// fetch is web-gated, so SSR renders an empty list and the client reconciles.
pub(super) fn use_bookmarks(uuid: String) -> BookmarksController {
    let list = use_signal(Vec::<Bookmark>::new);
    let fresh_id = use_signal(|| None::<i64>);
    let toast = use_signal(|| None::<BookmarkToast>);
    let uuid_sig = use_signal(|| uuid);

    use_effect(move || {
        let _uuid = uuid_sig.peek().clone();
        let mut _list = list;
        #[cfg(feature = "web")]
        spawn(async move {
            if let Ok(items) = crate::data::list_bookmarks("", &_uuid).await {
                // The unified table can also hold reader bookmarks (CFI
                // positions) for a dual-format book; the listen drawer is
                // audio-only, so drop any non-numeric position rather than
                // render it as 0:00 and seek to the start.
                _list.set(
                    items
                        .into_iter()
                        .filter(|b| b.position.parse::<f64>().is_ok())
                        .collect(),
                );
            }
        });
    });

    BookmarksController {
        list,
        fresh_id,
        toast,
        uuid: uuid_sig,
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
    fn bookmark_position_seconds_parses_decimal() {
        assert!((bookmark_position_seconds(&bm("123.5")) - 123.5).abs() < f64::EPSILON);
    }

    #[test]
    fn bookmark_position_seconds_defaults_to_zero_on_garbage() {
        assert_eq!(bookmark_position_seconds(&bm("epubcfi(/6/4)")), 0.0);
    }

    #[test]
    fn chapter_label_at_returns_generic_when_no_chapters() {
        assert_eq!(chapter_label_at(10.0, &[]), "Bookmark");
    }

    #[test]
    fn chapter_label_at_formats_titled_chapter() {
        let chs = vec![ch(0.0, "Intro"), ch(300.0, "Part One")];
        assert_eq!(chapter_label_at(420.0, &chs), "Ch. 2 \u{00b7} Part One");
    }

    #[test]
    fn chapter_label_at_falls_back_to_ordinal_for_untitled() {
        let chs = vec![ch(0.0, "")];
        assert_eq!(chapter_label_at(10.0, &chs), "Chapter 1");
    }
}
