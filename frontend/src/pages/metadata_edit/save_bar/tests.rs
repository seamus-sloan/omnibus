//! What the save bar claims about the editor's state — in particular that a
//! replaced cover is neither "No changes" nor a reason to leave Save disabled
//! (#2241).
//!
//! The bar's `Discard` link needs a parent router these harnesses don't
//! provide, so it renders as an error boundary here and its label is asserted
//! in `metadata_edit.spec.ts` instead.

use super::*;
use crate::test_support::render_in_vdom;

/// Mount a real [`SaveBar`] whose `cover_replaced` starts at `REPLACED`.
/// The dirty memos are pinned empty: this is about the one state the bar got
/// wrong, where nothing else on the page has been edited.
fn bar<const REPLACED: bool>() -> Element {
    let fields = use_memo(Vec::<&'static str>::new);
    rsx! {
        SaveBar {
            uuid: "book-uuid".to_string(),
            dirty: DirtyState {
                fields,
                count: use_memo(move || fields().len()),
                cover_replaced: use_signal(|| REPLACED),
            },
            status: SaveStatus {
                saving: use_signal(|| false),
                error: use_signal(|| None),
            },
            on_save: EventHandler::new(|()| {}),
        }
    }
}

#[test]
fn save_bar_reports_no_changes_and_disables_save_on_an_untouched_editor() {
    let html = render_in_vdom(bar::<false>);
    assert!(html.contains("No changes"));
    assert!(!html.contains("me-cover-replaced"));
    assert!(html.contains("disabled"));
}

#[test]
fn save_bar_reports_a_replaced_cover_and_lets_save_close_the_editor() {
    let html = render_in_vdom(bar::<true>);
    assert!(!html.contains("No changes"));
    assert!(html.contains("data-testid=\"me-cover-replaced\""));
    assert!(html.contains("Done"));
    assert!(!html.contains("disabled"));
}
