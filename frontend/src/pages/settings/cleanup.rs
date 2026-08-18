//! Library cleanup settings section — the feature's entry point: per-kind
//! suggestion counts, a link into each kind's review queue, and the button
//! that queues a detection pass. Web/server only, like the other admin
//! sections; the `AdminUser` extractor on the `cleanup/*` server functions is
//! the real authorization boundary.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{CleanupCounts, CleanupKind};

use crate::data::{self, server_error_message};
use crate::{use_server_url, Route};

/// The "Library cleanup" section of `/settings`: one row per cleanup kind with
/// its pending count and a Review link, plus "Run detection now".
#[component]
pub fn CleanupSection() -> Element {
    let is_admin = crate::use_is_admin();
    let server_url = use_server_url();

    let counts = use_signal(|| None::<Vec<(CleanupKind, CleanupCounts)>>);
    let error = use_signal(|| false);
    let status = use_signal(|| None::<String>);
    let in_flight = use_signal(|| false);
    // Bumped after a detection pass is queued so the counts effect re-runs.
    let generation = use_signal(|| 0u32);

    spawn_counts_fetch(server_url.clone(), counts, error, generation);
    let on_detect = detect_handler(server_url, status, in_flight, generation);

    rsx! {
        section { class: "card", "data-testid": "cleanup-card",
            h2 { "Library cleanup" }
            p { class: "subtitle",
                "Near-duplicate authors, series, and tags, plus book titles carrying filename cruft."
            }
            if is_admin() {
                CleanupCountsList { counts, error }
                div { class: "cleanup-actions",
                    button {
                        class: "btn",
                        "data-testid": "cleanup-detect",
                        disabled: in_flight(),
                        onclick: move |_| on_detect(()),
                        "Run detection now"
                    }
                }
                if let Some(message) = status() {
                    p { class: "settings-status", role: "status", "data-testid": "cleanup-detect-status",
                        "{message}"
                    }
                }
            } else {
                p { class: "settings-status error", "data-testid": "cleanup-forbidden",
                    "Administrator access is required to run library cleanup."
                }
            }
        }
    }
}

/// Refresh the per-kind counts on mount and whenever `generation` moves.
fn spawn_counts_fetch(
    server_url: String,
    mut counts: Signal<Option<Vec<(CleanupKind, CleanupCounts)>>>,
    mut error: Signal<bool>,
    generation: Signal<u32>,
) {
    use_effect(move || {
        // Read the generation inside the effect so a queued detection pass
        // re-subscribes this fetch rather than needing its own call site.
        let _ = generation();
        let server_url = server_url.clone();
        error.set(false);
        spawn(async move {
            match data::get_cleanup_counts(&server_url).await {
                Ok(loaded) => counts.set(Some(loaded)),
                Err(_) => error.set(true),
            }
        });
    });
}

/// Build the "Run detection now" click handler: queue a full detection pass,
/// report the outcome, and bump the counts generation.
fn detect_handler(
    server_url: String,
    status: Signal<Option<String>>,
    in_flight: Signal<bool>,
    generation: Signal<u32>,
) -> impl Fn(()) {
    move |()| {
        // `Signal` is `Copy`; re-binding here keeps the closure `Fn` (a
        // click handler is called by shared reference) while still writing.
        let (mut status, mut in_flight, mut generation) = (status, in_flight, generation);
        if in_flight() {
            return;
        }
        let server_url = server_url.clone();
        in_flight.set(true);
        status.set(None);
        spawn(async move {
            match data::run_cleanup_detection(&server_url, None).await {
                Ok(_) => {
                    status.set(Some(
                        "Detection started. Counts update as it finishes.".to_string(),
                    ));
                    generation.set(generation() + 1);
                }
                Err(e) => status.set(Some(server_error_message(&e))),
            }
            in_flight.set(false);
        });
    }
}

/// The counts region: loading / error / one row per kind.
#[component]
fn CleanupCountsList(
    counts: Signal<Option<Vec<(CleanupKind, CleanupCounts)>>>,
    error: Signal<bool>,
) -> Element {
    if error() {
        return rsx! {
            p { class: "settings-status error", role: "status", "data-testid": "cleanup-counts-error",
                "Failed to load cleanup counts."
            }
        };
    }
    let Some(rows) = counts() else {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "cleanup-counts-loading",
                "Loading\u{2026}"
            }
        };
    };
    rsx! {
        ul { class: "cleanup-kinds", "data-testid": "cleanup-kinds",
            for (kind, count) in rows {
                CleanupKindRow { key: "{kind.as_str()}", kind, count }
            }
        }
    }
}

/// One kind's row: its label, its pending count, and the Review link.
#[component]
fn CleanupKindRow(kind: CleanupKind, count: CleanupCounts) -> Element {
    rsx! {
        li { class: "cleanup-kind-row", "data-testid": "cleanup-kind-{kind.as_str()}",
            span { class: "cleanup-kind-label", "{kind_label(kind)}" }
            span {
                class: "cleanup-kind-count",
                "data-testid": "cleanup-count-{kind.as_str()}",
                "{count.pending} pending"
            }
            Link {
                to: Route::CleanupReview { kind: kind.as_str().to_string() },
                class: "btn",
                "data-testid": "cleanup-review-{kind.as_str()}",
                "Review"
            }
        }
    }
}

/// Human-readable name for a cleanup kind.
fn kind_label(kind: CleanupKind) -> &'static str {
    match kind {
        CleanupKind::Author => "Authors",
        CleanupKind::Series => "Series",
        CleanupKind::Tag => "Tags",
        CleanupKind::BookTitle => "Book titles",
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use omnibus_shared::{CleanupCounts, CleanupKind};

    use super::{kind_label, CleanupCountsList, CleanupKindRow};
    use crate::test_support::render_in_vdom;
    use dioxus::prelude::*;
    use dioxus_router::{Routable, Router};

    #[test]
    fn kind_label_names_every_cleanup_kind() {
        assert_eq!(kind_label(CleanupKind::Author), "Authors");
        assert_eq!(kind_label(CleanupKind::Series), "Series");
        assert_eq!(kind_label(CleanupKind::Tag), "Tags");
        assert_eq!(kind_label(CleanupKind::BookTitle), "Book titles");
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum RowRoute {
        #[route("/")]
        RowHost {},
    }

    // `CleanupKindRow` renders a router `Link`, which panics without a parent
    // router — so the row is mounted behind a one-route router.
    #[component]
    fn RowHost() -> Element {
        rsx! {
            CleanupKindRow {
                kind: CleanupKind::Tag,
                count: CleanupCounts { pending: 4, accepted: 1, rejected: 0 },
            }
        }
    }

    #[test]
    fn cleanup_kind_row_renders_the_label_pending_count_and_review_link() {
        let html = render_in_vdom(|| rsx! { Router::<RowRoute> {} });
        assert!(html.contains("Tags"));
        assert!(html.contains("4 pending"));
        assert!(html.contains("/settings/cleanup/tag"));
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum LoadingRoute {
        #[route("/")]
        LoadingHost {},
    }

    #[component]
    fn LoadingHost() -> Element {
        let counts = use_signal(|| None);
        let error = use_signal(|| false);
        rsx! { CleanupCountsList { counts, error } }
    }

    #[test]
    fn cleanup_counts_list_renders_the_loading_state_before_counts_arrive() {
        let html = render_in_vdom(|| rsx! { Router::<LoadingRoute> {} });
        assert!(html.contains("cleanup-counts-loading"));
    }
}
