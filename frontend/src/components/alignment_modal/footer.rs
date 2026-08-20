//! The footer action row: unlink (linked books only), cancel, and
//! confirm. The confirm handler is where a reorder and a mode pick turn
//! into the `ConfirmCrossFormatLink` payload that switches sync on.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink, CrossFormatLinkMode};

use crate::data;

/// Unlink / cancel / confirm. Reads `view`/`mode`/`primary`/`order` at
/// click time rather than closing over a snapshot, so a reorder made
/// after the last render still lands in the confirmed payload.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_footer(
    uuid: &str,
    mut busy: Signal<bool>,
    mut error: Signal<Option<String>>,
    mut open: Signal<bool>,
    on_changed: EventHandler<()>,
    view: Signal<Option<AlignmentView>>,
    mode: Signal<CrossFormatLinkMode>,
    primary: Signal<Option<i64>>,
    order: Signal<Vec<i64>>,
    has_link: bool,
    foot: Option<&'static str>,
    confirm_text: &'static str,
) -> Element {
    let unlink_uuid = uuid.to_string();
    let handle_unlink = move |_| {
        let uuid = unlink_uuid.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::unlink_cross_format("", &uuid).await {
                Ok(_) => {
                    busy.set(false);
                    open.set(false);
                    on_changed.call(());
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    let confirm_uuid = uuid.to_string();
    let handle_confirm = move |_| {
        let Some(v) = view() else { return };
        let original: Vec<i64> = v.audio_files.iter().map(|f| f.book_file_id).collect();
        let reordered = order() != original;
        let update = ConfirmCrossFormatLink {
            book_uuid: confirm_uuid.clone(),
            mode: mode(),
            primary_book_file_id: if mode() == CrossFormatLinkMode::Narrations {
                primary()
            } else {
                None
            },
            audio_order: if reordered && mode() == CrossFormatLinkMode::Sequence {
                Some(order())
            } else {
                None
            },
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::confirm_cross_format_link("", update).await {
                Ok(()) => {
                    busy.set(false);
                    open.set(false);
                    on_changed.call(());
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    rsx! {
        div { class: "al-foot",
            if let Some(note) = foot {
                span { class: "al-foot-note", "{note}" }
            }
            if has_link {
                button {
                    class: "btn ghost sm",
                    "data-testid": "alignment-unlink",
                    disabled: busy(),
                    onclick: handle_unlink,
                    "Unlink"
                }
            }
            span { class: "al-foot-spacer" }
            button {
                class: "btn ghost",
                "data-testid": "alignment-cancel",
                disabled: busy(),
                onclick: move |_| open.set(false),
                "Cancel"
            }
            // Full-size accent primary per the design.
            button {
                class: "btn primary",
                "data-testid": "alignment-confirm",
                disabled: busy() || view().is_none(),
                onclick: handle_confirm,
                "{confirm_text}"
            }
        }
    }
}
