//! Web-only merge dialog + post-merge toast for the book-detail page.
//!
//! Mobile build compiles to stubs returning `None`.

#[cfg(not(feature = "mobile"))]
use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::MergeBooksResult;

#[cfg(not(feature = "mobile"))]
use crate::components::MergeDialog;
#[cfg(not(feature = "mobile"))]
use crate::data;

/// Build the "Merge with…" rail button. Admin-only on web; never present on mobile.
#[cfg(not(feature = "mobile"))]
pub(super) fn build_merge_button(
    is_admin_flag: bool,
    mut merge_open: Signal<bool>,
) -> Option<Element> {
    is_admin_flag.then(|| {
        rsx! {
            button {
                class: "btn ghost sm bd-rail-edit",
                "data-testid": "merge-with",
                onclick: move |_| merge_open.set(true),
                "Merge with\u{2026}"
            }
        }
    })
}

/// Mobile stub for the rail button — always `None`.
#[cfg(feature = "mobile")]
pub(super) fn build_merge_button(_is_admin_flag: bool) -> Option<dioxus::prelude::Element> {
    None
}

/// Build the merge dialog + post-merge toast block. Web-only.
#[cfg(not(feature = "mobile"))]
pub(super) fn build_merge_ui(
    mut merge_open: Signal<bool>,
    mut merge_result: Signal<Option<MergeBooksResult>>,
    mut undo_error: Signal<Option<String>>,
    mut refresh: Signal<u32>,
    server_url: String,
    page_uuid: String,
    page_title: String,
) -> Option<Element> {
    let undo_url = server_url;
    Some(rsx! {
        if merge_open() {
            MergeDialog {
                target_uuid: page_uuid.clone(),
                target_title: page_title.clone(),
                on_merged: move |res: MergeBooksResult| {
                    merge_open.set(false);
                    undo_error.set(None);
                    merge_result.set(Some(res));
                    refresh.set(refresh() + 1);
                },
                on_close: move |_| merge_open.set(false),
            }
        }
        if let Some(res) = merge_result() {
            div { class: "bd-merge-toast card", role: "status",
                span { "Books merged." }
                button {
                    class: "btn ghost sm",
                    "data-testid": "merge-undo",
                    onclick: move |_| {
                        let url = undo_url.clone();
                        spawn(async move {
                            match data::undo_merge(&url, res.merge_log_id).await {
                                Ok(_) => {
                                    merge_result.set(None);
                                    refresh.set(refresh() + 1);
                                }
                                Err(e) => undo_error.set(Some(e.to_string())),
                            }
                        });
                    },
                    "Undo"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "merge-toast-dismiss",
                    aria_label: "Dismiss",
                    onclick: move |_| merge_result.set(None),
                    "\u{00d7}"
                }
                if let Some(e) = undo_error() {
                    p { role: "alert", class: "bd-merge-error", "{e}" }
                }
            }
        }
    })
}

/// Mobile stub for the merge UI — always `None`.
#[cfg(feature = "mobile")]
pub(super) fn build_merge_ui() -> Option<dioxus::prelude::Element> {
    None
}
