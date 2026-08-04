//! Reader bookmarks drawer. Saves the current page position as a CFI through
//! the unified bookmarks backend (shared with the audiobook player); each row
//! navigates the rendition back to its CFI.

use dioxus::prelude::*;
use omnibus_shared::Bookmark;
#[cfg(any(feature = "web", feature = "mobile"))]
use omnibus_shared::CreateBookmark;

use super::drawer_shell::ReaderDrawerShell;

#[component]
pub(super) fn ReaderBookmarksDrawer(
    uuid: String,
    current_cfi: String,
    current_label: String,
    on_navigate: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let list = use_signal(Vec::<Bookmark>::new);
    let server_url = crate::contexts::use_server_url();

    // Load existing bookmarks after mount (SSR renders empty for hydration
    // parity; the load effect is declared unconditionally).
    let uuid_load = uuid.clone();
    let server_load = server_url.clone();
    use_effect(move || {
        let _uuid = uuid_load.clone();
        let _server = server_load.clone();
        let mut _list = list;
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
            let mut list = list;
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

    let marks = list.read().clone();

    rsx! {
        ReaderDrawerShell {
            testid: "reader-bookmarks-drawer",
            on_close,
            head: rsx! {
                h4 { class: "rd-drawer-title",
                    "Bookmarks "
                    span { class: "rd-drawer-count", "{marks.len()}" }
                }
            },
            actions: rsx! {
                button {
                    class: "btn sm",
                    r#type: "button",
                    "data-testid": "reader-bookmark-add",
                    disabled: !can_add,
                    onclick: add,
                    "+ Bookmark"
                }
            },
            div { class: "rd-drawer-body",
                if marks.is_empty() {
                    div { class: "rd-drawer-empty",
                        "No bookmarks yet. Tap + Bookmark to save your place."
                    }
                } else {
                    for mark in marks.iter() {
                        ReaderBookmarkRow { key: "{mark.id}", bookmark: mark.clone(), on_navigate, list }
                    }
                }
            }
        }
    }
}

#[component]
fn ReaderBookmarkRow(
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
        div { class: "rd-bm-row", "data-testid": "reader-bookmark-row",
            button {
                class: "rd-bm-main",
                r#type: "button",
                onclick: move |_| on_navigate.call(cfi.clone()),
                span { class: "rd-bm-flag", "\u{2691}" }
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
