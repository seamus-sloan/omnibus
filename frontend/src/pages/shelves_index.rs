//! Shelves index page (`/shelves`) — a full-screen list of the caller's
//! shelves, the mobile design's primary shelf-navigation surface (reached from
//! the library header). Each row links to that shelf's detail page; the header
//! hosts a "New shelf" action. On web (which browses shelves via the rail) it
//! stands in as a plain list.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use crate::components::CreateShelfModal;
use crate::{data, use_server_url, Route};

/// Shelves index — see the module doc.
#[component]
pub fn ShelvesIndexPage() -> Element {
    let mut shelves = use_signal(Vec::<ShelfSummary>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut show_create = use_signal(|| false);
    let url = use_server_url();

    // Shared load path: surfaces failures instead of silently rendering an
    // empty list (matches the Authors/Tags index pages). `Copy` because it
    // captures only `Copy` signals, so both the mount effect and the
    // post-create refetch can call it.
    let load = move |url: String| {
        spawn(async move {
            if shelves.peek().is_empty() {
                loading.set(true);
            }
            match data::list_shelves(&url).await {
                Ok(s) => {
                    shelves.set(s);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    };

    let effect_url = url.clone();
    let generation = crate::use_cache_generation();
    use_effect(move || {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        load(effect_url.clone())
    });

    let refetch_url = url.clone();
    let refetch = move || load(refetch_url.clone());

    let count = shelves.read().len();

    rsx! {
        div { class: "m-shelves", "data-testid": "shelves-index",
            ShelvesHeader { count, on_new: move |_| show_create.set(true) }
            ShelvesBody { shelves: shelves(), loading: loading(), error: error() }
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

/// Back link, shelf count, "New" action, and page title.
#[component]
fn ShelvesHeader(count: usize, on_new: EventHandler<MouseEvent>) -> Element {
    let count_label = if count == 1 {
        "1 shelf".to_string()
    } else {
        format!("{count} shelves")
    };
    rsx! {
        header { class: "m-head",
            div { class: "m-head-top",
                div { class: "m-head-lead",
                    Link {
                        to: Route::Landing {},
                        class: "m-icon-btn m-head-back",
                        "aria-label": "Back to library",
                        "\u{2190}"
                    }
                    span { class: "label", "{count_label}" }
                }
                button {
                    r#type: "button",
                    class: "btn primary sm",
                    "data-testid": "new-shelf",
                    onclick: on_new,
                    "\u{FF0B} New"
                }
            }
            h2 { class: "m-head-title",
                "Your "
                span { class: "m-em", "shelves" }
            }
        }
    }
}

/// Error message, loading placeholder, or the shelf list — in priority order.
#[component]
fn ShelvesBody(shelves: Vec<ShelfSummary>, loading: bool, error: Option<String>) -> Element {
    rsx! {
        if let Some(msg) = error {
            p {
                role: "alert",
                class: "subtitle",
                "data-testid": "shelves-error",
                "Couldn't load shelves: {msg}"
            }
        } else if loading && shelves.is_empty() {
            p { class: "subtitle", "Loading\u{2026}" }
        } else {
            div { class: "m-shelf-list",
                // Always-present "All Books" — the whole library as a shelf.
                {all_books_row()}
                for s in shelves.iter() {
                    {shelf_row(s)}
                }
            }
        }
    }
}

/// Pinned "All Books" row — the whole library as a shelf, linking home.
fn all_books_row() -> Element {
    rsx! {
        Link {
            to: Route::Landing {},
            class: "m-shelf-row",
            "data-testid": "shelf-row-all",
            span { class: "m-shelf-icon", style: "--shelf-accent: var(--accent);",
                {manual_glyph()}
            }
            span { class: "m-shelf-row-body",
                span { class: "m-shelf-row-name", "All Books" }
                span { class: "m-shelf-row-sub", "Everything in your library" }
            }
            span { class: "m-shelf-row-chevron", {chevron()} }
        }
    }
}

/// One shelf row: accent-tinted glyph tile, name + kind, count, visibility.
fn shelf_row(s: &ShelfSummary) -> Element {
    let id = s.id;
    let accent = s.accent.clone().unwrap_or_else(|| "var(--accent)".into());
    let is_smart = s.kind == ShelfKind::Smart;
    let secondary = if is_smart {
        "Smart shelf"
    } else {
        "Hand-picked"
    };

    rsx! {
        Link {
            key: "{id}",
            to: Route::ShelfDetail { id },
            class: "m-shelf-row",
            "data-testid": "shelf-row-{id}",
            span {
                class: "m-shelf-icon",
                style: "--shelf-accent: {accent};",
                if is_smart { {smart_glyph()} } else { {manual_glyph()} }
            }
            span { class: "m-shelf-row-body",
                span { class: "m-shelf-row-name",
                    "{s.name}"
                    if is_smart {
                        span { class: "m-shelf-smart-mark", style: "color: {accent};",
                            {smart_mark()}
                        }
                    }
                }
                span { class: "m-shelf-row-sub", "{secondary}" }
            }
            span { class: "m-shelf-row-meta",
                span { class: "mono m-shelf-row-count", "{s.book_count}" }
                span { class: "m-shelf-row-vis",
                    match s.visibility {
                        Visibility::Private => rsx! { {lock_glyph()} },
                        Visibility::Public => rsx! { {people_glyph()} },
                    }
                }
            }
            span { class: "m-shelf-row-chevron", {chevron()} }
        }
    }
}

fn manual_glyph() -> Element {
    rsx! {
        svg {
            width: "20", height: "20", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" }
        }
    }
}

fn smart_glyph() -> Element {
    rsx! {
        svg {
            width: "20", height: "20", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M5 3v4M3 5h4M6 17v4M4 19h4" }
            path { d: "M13 3l2.5 6.5L22 12l-6.5 2.5L13 21l-2.5-6.5L4 12l6.5-2.5z" }
        }
    }
}

/// Small cog badge appended after a smart shelf's name.
fn smart_mark() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "3" }
            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
        }
    }
}

fn lock_glyph() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            rect { x: "3", y: "11", width: "18", height: "11", rx: "2", ry: "2" }
            path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
        }
    }
}

fn people_glyph() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            path { d: "M23 21v-2a4 4 0 0 0-3-3.87" }
            path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
        }
    }
}

fn chevron() -> Element {
    rsx! {
        svg {
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M9 18l6-6-6-6" }
        }
    }
}
