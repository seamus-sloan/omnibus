//! Left-rail shelf list — the shared library chrome for browse + shelf views.
//!
//! Lists the caller's visible shelves with a kind marker (cog for smart,
//! accent swatch for manual) and a visibility icon, plus an "All books" row
//! that returns to the landing grid. Mounts the create-shelf modal locally.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use crate::components::CreateShelfModal;
use crate::{data, use_server_url, Route};

/// Which rail row is currently active — drives the highlight.
#[derive(Clone, Copy, PartialEq)]
pub enum RailActive {
    /// The "All books" row (landing page).
    All,
    /// A specific shelf's detail page.
    Shelf(i64),
}

/// The shelf rail. Fetches the caller's shelves on mount (SSR-safe: starts
/// empty so the first WASM paint matches the SSR markup) and renders one row
/// per shelf plus the "All books" entry.
#[component]
pub fn ShelvesRail(active: RailActive) -> Element {
    let mut shelves = use_signal(Vec::<ShelfSummary>::new);
    let mut show_create = use_signal(|| false);
    let url = use_server_url();
    // `None` until the boot effect resolves the viewer (SSR + first paint).
    let viewer_id = crate::use_current_user_summary()().map(|u| u.id);

    let refetch_url = url.clone();
    let refetch = move || {
        let url = refetch_url.clone();
        spawn(async move {
            if let Ok(s) = data::list_shelves(&url).await {
                shelves.set(s);
            }
        });
    };

    // Mount fetch reuses `refetch` rather than re-implementing its body —
    // the two copies previously drifted independently (#1337).
    use_effect(refetch.clone());

    let all_active = matches!(active, RailActive::All);
    let all_class = if all_active {
        "shelf-row shelf-row--active"
    } else {
        "shelf-row"
    };

    rsx! {
        aside { class: "shelf-rail", "data-testid": "shelves-rail",
            div { class: "shelf-rail-head",
                span { class: "label", "Shelves" }
                button {
                    r#type: "button",
                    class: "shelf-rail-new",
                    "data-testid": "new-shelf",
                    onclick: move |_| show_create.set(true),
                    "\u{FF0B} New shelf"
                }
            }

            nav { class: "shelf-rail-list", aria_label: "Shelves",
                Link {
                    to: Route::Landing {},
                    class: "{all_class}",
                    "data-testid": "rail-all-books",
                    span { class: "shelf-row-marker", {all_books_icon()} }
                    span { class: "shelf-row-name", "All books" }
                }

                for s in shelves.read().iter() {
                    {render_shelf_row(s, active, viewer_id)}
                }
            }

            p { class: "shelf-rail-foot mono", "Smart shelves update on their own" }
        }

        if show_create() {
            CreateShelfModal {
                on_close: move |_| show_create.set(false),
                on_created: move |_| {
                    show_create.set(false);
                    refetch();
                },
            }
        }
    }
}

/// One shelf row: kind marker, name (+ owner attribution when not yours),
/// count, visibility icon.
fn render_shelf_row(s: &ShelfSummary, active: RailActive, viewer_id: Option<i64>) -> Element {
    let id = s.id;
    let is_active = matches!(active, RailActive::Shelf(other) if other == id);
    let class = if is_active {
        "shelf-row shelf-row--active"
    } else {
        "shelf-row"
    };
    let accent = s.accent.clone().unwrap_or_else(|| "var(--accent)".into());
    // Attribute a shelf that isn't the viewer's (only once the viewer is known).
    // A wishlist's system name already opens with the owner's username, so the
    // chip would just repeat it.
    let not_mine =
        viewer_id.is_some_and(|vid| vid != s.owner_user_id) && s.kind != ShelfKind::Wishlist;

    rsx! {
        Link {
            key: "{id}",
            to: Route::ShelfDetail { id },
            class: "{class}",
            "data-testid": "rail-shelf-{id}",
            span { class: "shelf-row-marker",
                match s.kind {
                    ShelfKind::Smart => rsx! { {cog_icon()} },
                    ShelfKind::Wishlist => rsx! { {heart_icon()} },
                    ShelfKind::Manual => rsx! {
                        span {
                            class: "shelf-row-swatch",
                            style: "background: {accent};",
                        }
                    },
                }
            }
            span { class: "shelf-row-name",
                "{s.name}"
                if not_mine {
                    span { class: "shelf-row-owner mono", "by {s.owner_username}" }
                }
            }
            span { class: "shelf-row-count mono", "{s.book_count}" }
            span { class: "shelf-row-vis",
                match s.visibility {
                    Visibility::Private => rsx! { {lock_icon()} },
                    Visibility::Public => rsx! { {people_icon()} },
                }
            }
        }
    }
}

/// Stack-of-books glyph for the "All books" row.
fn all_books_icon() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
        }
    }
}

/// Cog glyph marking a smart shelf.
fn cog_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "3" }
            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
        }
    }
}

/// Heart glyph marking the built-in Wishlist shelf.
fn heart_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" }
        }
    }
}

/// Padlock glyph marking a private shelf.
fn lock_icon() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            rect { x: "3", y: "11", width: "18", height: "11", rx: "2", ry: "2" }
            path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
        }
    }
}

/// Two-people glyph marking a public shelf.
fn people_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            path { d: "M23 21v-2a4 4 0 0 0-3-3.87" }
            path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
        }
    }
}
