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

/// The rail button's label. "Delete files…" is a promise about the
/// filesystem, and a wishlist entry or paper-only book has no files to make it
/// about — the dialog behind it says so outright ("This book has no files on
/// disk"), which is the tell that the *label* was wrong, not the control
/// (#2471).
///
/// The control itself stays on a fileless record, because it is that record's
/// only removal: the dialog's PHYSICAL COPIES section is how a paper-only book
/// is un-recorded, and deleting its last item deletes the record. Gating the
/// button away took that path with it.
#[cfg(not(feature = "mobile"))]
fn delete_label(has_files: bool) -> &'static str {
    if has_files {
        "Delete files\u{2026}"
    } else {
        "Delete record\u{2026}"
    }
}

/// Build the delete rail button. Admin-only on web; never present on mobile.
#[cfg(not(feature = "mobile"))]
pub(super) fn build_delete_button(
    is_admin_flag: bool,
    mut delete_open: Signal<bool>,
    has_files: bool,
) -> Option<Element> {
    is_admin_flag.then(|| {
        rsx! {
            button {
                class: "btn ghost sm bd-rail-edit bd-rail-delete",
                // Testid unchanged across the relabel: it names the control,
                // and every spec reaching the delete dialog goes through it.
                "data-testid": "delete-files",
                onclick: move |_| delete_open.set(true),
                {delete_label(has_files)}
            }
        }
    })
}

/// Mobile stub for the rail button — always `None`.
#[cfg(feature = "mobile")]
pub(super) fn build_delete_button(_is_admin_flag: bool) -> Option<dioxus::prelude::Element> {
    None
}

#[cfg(all(test, not(feature = "mobile")))]
mod label_tests {
    use super::delete_label;

    // Regression for #2471: a record with nothing on disk was offering to
    // delete files.
    #[test]
    fn delete_label_promises_files_only_when_there_are_files() {
        assert_eq!(delete_label(true), "Delete files\u{2026}");
        assert_eq!(delete_label(false), "Delete record\u{2026}");
    }
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
