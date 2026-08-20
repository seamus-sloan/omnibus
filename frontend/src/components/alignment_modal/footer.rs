//! The footer action row: unlink (linked books only), cancel, and
//! confirm. The confirm handler is where a reorder and a mode pick turn
//! into the `ConfirmCrossFormatLink` payload that switches sync on.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink, CrossFormatLinkMode};

use crate::data;

/// Builds the `ConfirmCrossFormatLink` payload from the current mode and
/// working order. A reorder only rides along in `Sequence` mode — the
/// server has nothing to reorder in `Narrations`, where each file is its
/// own complete book — and only when it actually differs from `original`,
/// the order the view was served with.
fn build_confirm_payload(
    uuid: &str,
    mode: CrossFormatLinkMode,
    primary: Option<i64>,
    order: Vec<i64>,
    original: &[i64],
) -> ConfirmCrossFormatLink {
    let reordered = order != original;
    ConfirmCrossFormatLink {
        book_uuid: uuid.to_string(),
        mode,
        primary_book_file_id: if mode == CrossFormatLinkMode::Narrations {
            primary
        } else {
            None
        },
        audio_order: if reordered && mode == CrossFormatLinkMode::Sequence {
            Some(order)
        } else {
            None
        },
    }
}

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
        let update = build_confirm_payload(&confirm_uuid, mode(), primary(), order(), &original);
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

#[cfg(test)]
mod tests {
    use omnibus_shared::CrossFormatLinkMode;

    use super::build_confirm_payload;

    #[test]
    fn build_confirm_payload_carries_the_primary_and_drops_order_in_narrations_mode() {
        let payload = build_confirm_payload(
            "book-uuid",
            CrossFormatLinkMode::Narrations,
            Some(7),
            vec![7, 8, 9],
            &[9, 8, 7],
        );
        assert_eq!(payload.book_uuid, "book-uuid");
        assert_eq!(payload.mode, CrossFormatLinkMode::Narrations);
        assert_eq!(payload.primary_book_file_id, Some(7));
        // Narrations has nothing to reorder server-side, even though the
        // working order here does differ from `original`.
        assert_eq!(payload.audio_order, None);
    }

    #[test]
    fn build_confirm_payload_carries_a_changed_order_and_drops_primary_in_sequence_mode() {
        let payload = build_confirm_payload(
            "book-uuid",
            CrossFormatLinkMode::Sequence,
            Some(7),
            vec![9, 8, 7],
            &[7, 8, 9],
        );
        assert_eq!(payload.mode, CrossFormatLinkMode::Sequence);
        // Narrations-only field; Sequence never sends a primary.
        assert_eq!(payload.primary_book_file_id, None);
        assert_eq!(payload.audio_order, Some(vec![9, 8, 7]));
    }

    #[test]
    fn build_confirm_payload_omits_order_when_it_matches_the_served_original() {
        let payload = build_confirm_payload(
            "book-uuid",
            CrossFormatLinkMode::Sequence,
            None,
            vec![1, 2, 3],
            &[1, 2, 3],
        );
        // Nothing moved, so there is nothing to save — the confirm is a
        // bare "turn sync on" with no order in the payload.
        assert_eq!(payload.audio_order, None);
    }
}
