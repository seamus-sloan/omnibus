//! Chrome around the review card: the breadcrumb back into Settings, the
//! serif heading with its pass-progress readout, and the kind line that says
//! where this queue sits among the other three. Split from the page module so
//! that file stays about the queue rather than the furniture.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{CleanupCounts, CleanupKind};

use crate::Route;

/// Every kind, in `CleanupKind`'s own declaration order — the order the
/// counts RPC returns, so the chips never reshuffle between refreshes.
pub const KINDS: [CleanupKind; 4] = [
    CleanupKind::Author,
    CleanupKind::Series,
    CleanupKind::Tag,
    CleanupKind::BookTitle,
];

/// Settings › Library cleanup › Review <kind>.
#[component]
pub fn CleanupCrumb(kind: Option<CleanupKind>) -> Element {
    rsx! {
        nav { class: "bd-crumb crx-crumb", "aria-label": "breadcrumb",
            Link { to: Route::Settings { section: None }, class: "bd-crumb-home", "Settings" }
            span { class: "bd-crumb-sep", "\u{203a}" }
            Link {
                to: Route::Settings { section: Some("cleanup".to_string()) },
                class: "bd-crumb-step",
                "Library cleanup"
            }
            span { class: "bd-crumb-sep", "\u{203a}" }
            span { class: "bd-crumb-curr", "{kind_title(kind)}" }
        }
    }
}

/// "3 of 9" plus one dot per card in the loaded page.
///
/// `total` is the page the queue handed back, not the whole backlog — a
/// backlog past `REVIEW_QUEUE_MAX` is worked through a page at a time, and a
/// dot row claiming otherwise would promise an end that isn't there.
#[component]
pub fn CleanupProgress(index: usize, total: usize) -> Element {
    let position = (index + 1).min(total.max(1));
    rsx! {
        div { class: "crx-progress", "data-testid": "cleanup-progress",
            span { class: "crx-progress-text", "{position} of {total}" }
            div { class: "crx-dots",
                for slot in 0..total {
                    span {
                        key: "{slot}",
                        class: "crx-dot {dot_state(slot, index)}",
                    }
                }
            }
        }
    }
}

/// Which dot this slot is: the one under review, one already passed, or one
/// still ahead.
fn dot_state(slot: usize, index: usize) -> &'static str {
    match slot.cmp(&index) {
        std::cmp::Ordering::Less => "done",
        std::cmp::Ordering::Equal => "on",
        std::cmp::Ordering::Greater => "",
    }
}

/// One chip per kind with its pending count, linking into that kind's queue.
/// Counts arrive separately from the queue, so the chips render without them
/// rather than holding the card back on a second request.
#[component]
pub fn CleanupKindLine(
    current: Option<CleanupKind>,
    counts: ReadSignal<Option<Vec<(CleanupKind, CleanupCounts)>>>,
) -> Element {
    let loaded = counts();
    rsx! {
        div { class: "crx-kindline", "data-testid": "cleanup-kindline",
            for kind in KINDS {
                Link {
                    key: "{kind.as_str()}",
                    to: Route::CleanupReview { kind: kind.as_str().to_string() },
                    class: if Some(kind) == current { "crx-kindchip on" } else { "crx-kindchip" },
                    "data-testid": "cleanup-kindchip-{kind.as_str()}",
                    "{kind_label(kind)}"
                    if let Some(pending) = pending_for(&loaded, kind) {
                        span { class: "crx-kind-count", "{pending}" }
                    }
                }
            }
        }
    }
}

/// This kind's pending count, or `None` while the counts are still in flight.
fn pending_for(
    counts: &Option<Vec<(CleanupKind, CleanupCounts)>>,
    kind: CleanupKind,
) -> Option<i64> {
    counts
        .as_ref()?
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, c)| c.pending)
}

/// Short label for a kind chip.
pub fn kind_label(kind: CleanupKind) -> &'static str {
    match kind {
        CleanupKind::Author => "Authors",
        CleanupKind::Series => "Series",
        CleanupKind::Tag => "Tags",
        CleanupKind::BookTitle => "Book titles",
    }
}

/// Heading for the kind under review.
pub fn kind_title(kind: Option<CleanupKind>) -> &'static str {
    match kind {
        Some(CleanupKind::Author) => "Review authors",
        Some(CleanupKind::Series) => "Review series",
        Some(CleanupKind::Tag) => "Review tags",
        Some(CleanupKind::BookTitle) => "Review book titles",
        None => "Review library cleanup",
    }
}
