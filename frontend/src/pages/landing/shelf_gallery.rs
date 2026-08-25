//! The shelves row below the stack, drawn text first: names as type on a
//! hairline rule with up to three member covers peeking in above the name on
//! hover, which costs ~110px instead of the 170px the cover-mosaic tiles took.
//! Selecting filters the landing book list in place; nothing here navigates.
//! Horizontal paging (the `‹`/`›` arrows) is driven by `marquee.js`.

use dioxus::prelude::*;
use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use crate::components::shelves_rail::{cog_icon, heart_icon};
use crate::components::CreateShelfModal;
use crate::shelf_selection::ShelfSelection;

/// How many member covers peek in above a shelf name on hover. Three reads as
/// a hint of the shelf without becoming a mosaic again.
pub(super) const PEEK_COVERS: usize = 3;

/// Caption meta line: `"N books"`, plus `Public` when the shelf is shared.
/// Kind is carried by the badge glyph beside it rather than a second word.
pub(super) fn shelf_meta_line(count: i64, visibility: Visibility) -> String {
    let books = if count == 1 {
        "1 book".to_string()
    } else {
        format!("{count} books")
    };
    if visibility == Visibility::Public {
        format!("{books} \u{00b7} Public")
    } else {
        books
    }
}

/// Spoken label for a shelf entry. The kind badge is a decorative glyph, so
/// "Smart" / "Wishlist" has to reach a screen reader through the label.
pub(super) fn shelf_aria_label(name: &str, count: i64, kind: ShelfKind, vis: Visibility) -> String {
    let kind_word = match kind {
        ShelfKind::Smart => "Smart shelf ",
        ShelfKind::Wishlist => "Wishlist ",
        ShelfKind::Manual => "",
    };
    format!("{kind_word}{name}, {}", shelf_meta_line(count, vis))
}

/// The slab line above the row, which doubles as the receipt for what the
/// current pick is doing to the list below.
pub(super) fn slab_line(filtering: bool) -> &'static str {
    if filtering {
        "Shelves \u{2014} filtering the list below"
    } else {
        "Shelves \u{2014} showing everything"
    }
}

/// Per-render snapshot of the row's data + selection state: the shelf list,
/// which one is picked, the "All Books" entry's count/covers, the server URL
/// for thumbnails, and the select/create callbacks.
#[derive(Clone, PartialEq, Props)]
pub(super) struct ShelfGalleryProps {
    pub shelves: Vec<ShelfSummary>,
    pub selection: ShelfSelection,
    pub all_count: Option<i64>,
    pub all_cover_uuids: Vec<String>,
    pub server_url: String,
    pub on_select: EventHandler<ShelfSelection>,
    pub on_created: EventHandler<()>,
}

/// The row. Entries are toggle buttons (`aria-pressed` marks the pick); the
/// create-shelf modal mounts locally, same wiring as the shelf rail.
#[component]
pub(super) fn ShelfGallery(props: ShelfGalleryProps) -> Element {
    let ShelfGalleryProps {
        shelves,
        selection,
        all_count,
        all_cover_uuids,
        server_url,
        on_select,
        on_created,
    } = props;
    let mut show_create = use_signal(|| false);
    let all_active = selection == ShelfSelection::All;
    let all_meta = match all_count {
        Some(1) => "1 book".to_string(),
        Some(n) => format!("{n} books"),
        None => "Your whole library".to_string(),
    };
    rsx! {
        section {
            class: "lmq-shelves",
            "data-testid": "shelf-gallery",
            aria_label: "Shelves",
            div { class: "lmq-slab",
                span { class: "k", "{slab_line(!all_active)}" }
                button {
                    r#type: "button",
                    class: "lmq-slink",
                    "data-testid": "new-shelf",
                    onclick: move |_| show_create.set(true),
                    "\u{FF0B} New shelf"
                }
            }
            div { class: "lmq-shwrap",
                // Both arrows are always in the DOM and always inert until
                // `marquee.js` arms the end that has more to show — a
                // conditionally-rendered arrow would change the row's element
                // count between SSR and hydration (rule 07).
                button {
                    r#type: "button",
                    class: "lmq-shnav lmq-shnav--l",
                    "data-testid": "shelf-row-prev",
                    aria_label: "Earlier shelves",
                    tabindex: "-1",
                    "\u{2039}"
                }
                button {
                    r#type: "button",
                    class: "lmq-shnav lmq-shnav--r",
                    "data-testid": "shelf-row-next",
                    aria_label: "More shelves",
                    tabindex: "-1",
                    "\u{203A}"
                }
                div { class: "lmq-shtx", id: "lmq-shelf-row",
                    button {
                        r#type: "button",
                        class: "lmq-shent",
                        "data-testid": "gallery-all-books",
                        aria_pressed: all_active,
                        aria_label: "All Books, {all_meta}",
                        onclick: move |_| on_select.call(ShelfSelection::All),
                        {peek(&all_cover_uuids, &server_url, None)}
                        span { class: "lmq-shent-name", "All Books" }
                        span { class: "lmq-shent-meta", "{all_meta}" }
                    }
                    for s in shelves.iter() {
                        ShelfRowEntry {
                            key: "{s.id}",
                            shelf: s.clone(),
                            active: selection == ShelfSelection::Shelf(s.id),
                            server_url: server_url.clone(),
                            on_select,
                        }
                    }
                }
            }
        }

        if show_create() {
            CreateShelfModal {
                on_close: move |_| show_create.set(false),
                on_created: move |_| {
                    show_create.set(false);
                    on_created.call(());
                },
            }
        }
    }
}

/// One shelf entry: hover peek, name, kind badge + meta line.
#[component]
fn ShelfRowEntry(
    shelf: ShelfSummary,
    active: bool,
    server_url: String,
    on_select: EventHandler<ShelfSelection>,
) -> Element {
    let id = shelf.id;
    let meta = shelf_meta_line(shelf.book_count, shelf.visibility);
    let label = shelf_aria_label(&shelf.name, shelf.book_count, shelf.kind, shelf.visibility);
    let badge = match shelf.kind {
        ShelfKind::Smart => Some(cog_icon()),
        ShelfKind::Wishlist => Some(heart_icon()),
        ShelfKind::Manual => None,
    };
    rsx! {
        button {
            r#type: "button",
            class: "lmq-shent",
            "data-testid": "gallery-shelf-{id}",
            aria_pressed: active,
            aria_label: "{label}",
            onclick: move |_| on_select.call(ShelfSelection::Shelf(id)),
            {peek(&shelf.cover_uuids, &server_url, badge.clone())}
            span { class: "lmq-shent-name", "{shelf.name}" }
            span { class: "lmq-shent-meta",
                if let Some(glyph) = badge {
                    {glyph}
                }
                "{meta}"
            }
        }
    }
}

/// The covers that slide in above a name on hover — or, for a shelf with no
/// cover-bearing member, a single dashed plate carrying the kind glyph so the
/// row keeps its rhythm.
fn peek(cover_uuids: &[String], server_url: &str, glyph: Option<Element>) -> Element {
    if cover_uuids.is_empty() {
        return rsx! {
            span { class: "lmq-shent-peek", aria_hidden: true,
                span { class: "lmq-shent-none",
                    if let Some(glyph) = glyph {
                        {glyph}
                    }
                }
            }
        };
    }
    rsx! {
        span { class: "lmq-shent-peek", aria_hidden: true,
            for uuid in cover_uuids.iter().take(PEEK_COVERS) {
                span { key: "{uuid}",
                    img {
                        src: crate::thumb_url(server_url, uuid, "sm"),
                        alt: "",
                        loading: "lazy",
                        draggable: false,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
