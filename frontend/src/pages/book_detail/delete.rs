//! Web-only delete dialog + post-delete feedback for the book-detail page.
//!
//! Mobile build compiles to stubs returning `None`.

#[cfg(not(feature = "mobile"))]
use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::hooks::use_navigator;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::DeleteBookFilesResult;

#[cfg(not(feature = "mobile"))]
use crate::components::DeleteBookDialog;
#[cfg(not(feature = "mobile"))]
use crate::Route;

/// Build the "Delete files…" rail button. Admin-only on web; never present on mobile.
#[cfg(not(feature = "mobile"))]
pub(super) fn build_delete_button(
    is_admin_flag: bool,
    mut delete_open: Signal<bool>,
) -> Option<Element> {
    is_admin_flag.then(|| {
        rsx! {
            button {
                class: "btn ghost sm bd-rail-edit bd-rail-delete",
                "data-testid": "delete-files",
                onclick: move |_| delete_open.set(true),
                "Delete files\u{2026}"
            }
        }
    })
}

/// Mobile stub for the rail button — always `None`.
#[cfg(feature = "mobile")]
pub(super) fn build_delete_button(_is_admin_flag: bool) -> Option<dioxus::prelude::Element> {
    None
}

/// Build the delete dialog block. Web-only.
///
/// A partial delete bumps `refresh` so the page refetches without its deleted
/// file; a total delete leaves nothing to refetch, so it navigates back to the
/// landing grid instead.
#[cfg(not(feature = "mobile"))]
pub(super) fn build_delete_ui(
    mut delete_open: Signal<bool>,
    mut refresh: Signal<u32>,
    uuid: String,
    title: String,
) -> Option<Element> {
    let nav = use_navigator();
    Some(rsx! {
        if delete_open() {
            DeleteBookDialog {
                uuid: uuid.clone(),
                title: title.clone(),
                on_deleted: move |res: DeleteBookFilesResult| {
                    delete_open.set(false);
                    if res.book_deleted {
                        nav.push(Route::Landing {});
                    } else {
                        refresh.set(refresh() + 1);
                    }
                },
                on_close: move |_| delete_open.set(false),
            }
        }
    })
}

/// Mobile stub for the delete UI — always `None`.
#[cfg(feature = "mobile")]
pub(super) fn build_delete_ui() -> Option<dioxus::prelude::Element> {
    None
}
