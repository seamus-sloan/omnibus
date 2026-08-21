//! Library cleanup settings section — the feature's entry point: per-kind
//! suggestion counts, a link into each kind's review queue, and the button
//! that queues a detection pass. Web/server only, like the other admin
//! sections; the `AdminUser` extractor on the `cleanup/*` server functions is
//! the real authorization boundary.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{CleanupCounts, CleanupKind, IgnoredAuthor};

use crate::components::AuthorPicker;
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

    spawn_counts_fetch(is_admin, server_url.clone(), counts, error, generation);
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
                IgnoredAuthorsList {}
            } else {
                p { class: "settings-status error", "data-testid": "cleanup-forbidden",
                    "Administrator access is required to run library cleanup."
                }
            }
        }
    }
}

/// Refresh the per-kind counts once the visitor is known to be an admin, and
/// again whenever `generation` moves.
fn spawn_counts_fetch(
    is_admin: ReadSignal<bool>,
    server_url: String,
    mut counts: Signal<Option<Vec<(CleanupKind, CleanupCounts)>>>,
    mut error: Signal<bool>,
    generation: Signal<u32>,
) {
    use_effect(move || {
        // Both reads happen inside the effect so it re-subscribes to each: the
        // generation so a queued detection pass refreshes the counts, and
        // `is_admin` so the fetch waits for `CurrentUser` to resolve instead of
        // 403ing against the admin-gated route on the first paint.
        let _ = generation();
        if !is_admin() {
            return;
        }
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

/// Signals the ignored-authors list threads through its handlers, grouped
/// so the row renderer takes one argument instead of five.
#[derive(Clone, Copy, PartialEq)]
struct IgnoredListState {
    entries: Signal<Option<Vec<IgnoredAuthor>>>,
    /// Name whose "alias to…" picker is open, if any.
    converting: Signal<Option<String>>,
    status: Signal<Option<String>>,
    busy: Signal<bool>,
    /// Bumped after a convert/remove so the entries effect refetches.
    generation: Signal<u32>,
}

/// The `ignored_authors` blocklist manager: a name deleted as junk stays
/// here and is silently skipped on every scan, so a duplicate spelling
/// deleted by mistake permanently orphans its books. Each entry can be
/// converted into an alias onto a canonical author (the recovery path) or
/// removed outright; both queue the authorless relink pass server-side.
#[component]
fn IgnoredAuthorsList() -> Element {
    let server_url = use_server_url();
    let state = IgnoredListState {
        entries: use_signal(|| None),
        converting: use_signal(|| None),
        status: use_signal(|| None),
        busy: use_signal(|| false),
        generation: use_signal(|| 0u32),
    };

    let mut entries = state.entries;
    let generation = state.generation;
    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let _ = generation();
        let server_url = url_for_fetch.clone();
        spawn(async move {
            match data::get_ignored_authors(&server_url).await {
                Ok(list) => entries.set(Some(list)),
                Err(_) => entries.set(Some(Vec::new())),
            }
        });
    });

    let rows = entries();
    rsx! {
        div { class: "cleanup-ignored", "data-testid": "cleanup-ignored",
            h3 { "Ignored authors" }
            p { class: "subtitle",
                "Names skipped on every library scan (written by \"Delete author\"). \
                 Convert an entry to point a duplicate spelling at the real author, \
                 or remove it to let scans re-create the name."
            }
            match rows {
                None => rsx! {
                    p { class: "settings-status", role: "status", "data-testid": "cleanup-ignored-loading",
                        "Loading\u{2026}"
                    }
                },
                Some(list) if list.is_empty() => rsx! {
                    p { class: "settings-status", "data-testid": "cleanup-ignored-empty",
                        "No ignored authors."
                    }
                },
                Some(list) => rsx! {
                    ul { class: "cleanup-ignored-list", "data-testid": "cleanup-ignored-list",
                        for entry in list {
                            {ignored_author_row(server_url.clone(), entry.name.clone(), state)}
                        }
                    }
                },
            }
            if let Some(message) = (state.status)() {
                p { class: "settings-status", role: "status", "data-testid": "cleanup-ignored-status",
                    "{message}"
                }
            }
        }
    }
}

/// One blocklist row: the name, the Convert/Remove actions, and (when this
/// row's convert is armed) the canonical-author picker.
fn ignored_author_row(server_url: String, name: String, state: IgnoredListState) -> Element {
    let mut converting = state.converting;
    let busy = (state.busy)();
    let picker_open = converting().as_deref() == Some(name.as_str());
    let toggle_name = name.clone();
    let remove_name = name.clone();
    let pick_name = name.clone();
    let remove_url = server_url.clone();
    rsx! {
        li { key: "{name}", class: "cleanup-ignored-row", "data-testid": "cleanup-ignored-row",
            span { class: "cleanup-ignored-name", "{name}" }
            button {
                class: "btn",
                "data-testid": "cleanup-ignored-alias-btn",
                disabled: busy,
                onclick: move |_| {
                    let armed = converting().as_deref() == Some(toggle_name.as_str());
                    converting.set(if armed { None } else { Some(toggle_name.clone()) });
                },
                "Convert to alias\u{2026}"
            }
            button {
                class: "btn",
                "data-testid": "cleanup-ignored-remove-btn",
                disabled: busy,
                onclick: move |_| remove_ignored(remove_url.clone(), remove_name.clone(), state),
                "Remove"
            }
            if picker_open {
                div { class: "cleanup-ignored-picker", "data-testid": "cleanup-ignored-picker",
                    p { class: "subtitle", "Pick the author this spelling should resolve to:" }
                    AuthorPicker {
                        testid: "cleanup-ignored-author-picker".to_string(),
                        on_pick: {
                            let server_url = server_url.clone();
                            move |pick: crate::components::AuthorPick| {
                                convert_ignored(server_url.clone(), pick_name.clone(), pick.id, state)
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Convert `name` into an alias onto `canonical_id`, report, and refetch.
fn convert_ignored(server_url: String, name: String, canonical_id: i64, state: IgnoredListState) {
    let IgnoredListState {
        mut converting,
        mut status,
        mut busy,
        mut generation,
        ..
    } = state;
    if busy() {
        return;
    }
    busy.set(true);
    status.set(None);
    spawn(async move {
        match data::alias_ignored_author(&server_url, name, canonical_id).await {
            Ok(_) => {
                status.set(Some(
                    "Converted to alias. Relinking affected books in the background.".to_string(),
                ));
                converting.set(None);
                generation.set(generation() + 1);
            }
            Err(e) => status.set(Some(server_error_message(&e))),
        }
        busy.set(false);
    });
}

/// Remove `name` from the blocklist outright, report, and refetch.
fn remove_ignored(server_url: String, name: String, state: IgnoredListState) {
    let IgnoredListState {
        mut status,
        mut busy,
        mut generation,
        ..
    } = state;
    if busy() {
        return;
    }
    busy.set(true);
    status.set(None);
    spawn(async move {
        match data::remove_ignored_author(&server_url, name).await {
            Ok(()) => {
                status.set(Some(
                    "Removed. Relinking affected books in the background.".to_string(),
                ));
                generation.set(generation() + 1);
            }
            Err(e) => status.set(Some(server_error_message(&e))),
        }
        busy.set(false);
    });
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

    #[test]
    fn ignored_authors_list_renders_its_heading_and_loading_state_before_entries_arrive() {
        // SSR / first paint: effects never run, so the entries are still
        // `None` — hydration-parity contract of rule 07.
        let html = render_in_vdom(|| rsx! { super::IgnoredAuthorsList {} });
        assert!(html.contains("Ignored authors"));
        assert!(html.contains("cleanup-ignored-loading"));
    }
}
