//! Horizontal shelf gallery below the landing hero, mirroring the iOS
//! `ShelvesRail`/`ShelfCard` look: 2:3 cover-mosaic tiles with name + count
//! captions, a dashed "New shelf" tile, and — web-specific — an "All Books"
//! first tile plus a glowing outline on the selected tile. Selecting filters
//! the landing book list in place; nothing here navigates.

use dioxus::prelude::*;
use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use crate::components::shelves_rail::{all_books_icon, cog_icon, heart_icon};
use crate::components::CreateShelfModal;
use crate::shelf_selection::ShelfSelection;

/// How the 2:3 mosaic arranges its cover cells; mirrors the iOS
/// `ShelfMosaic` so no arrangement leaves a blank quadrant.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum MosaicLayout {
    Empty,
    One,
    Two,
    Three,
    Four,
}

/// Arrangement for `cover_count` covers (counts above four clamp to four —
/// the backend caps `cover_uuids` there anyway).
pub(super) fn mosaic_layout(cover_count: usize) -> MosaicLayout {
    match cover_count {
        0 => MosaicLayout::Empty,
        1 => MosaicLayout::One,
        2 => MosaicLayout::Two,
        3 => MosaicLayout::Three,
        _ => MosaicLayout::Four,
    }
}

/// djb2 hash of the shelf name folded to a hue, so accent-less shelves get a
/// stable plate color. Mirrors the iOS fallback (`hash % 360`).
pub(super) fn fallback_hue(name: &str) -> u32 {
    let mut hash: u32 = 5381;
    for b in name.bytes() {
        hash = hash.wrapping_mul(33) ^ u32::from(b);
    }
    hash % 360
}

/// CSS background for the empty-mosaic plate: a diagonal dark gradient from
/// the shelf accent when set, else from the name-hash hue (iOS
/// `ShelfMosaic.emptyPlate` equivalent).
pub(super) fn plate_gradient(accent: Option<&str>, name: &str) -> String {
    match accent {
        Some(accent) if !accent.trim().is_empty() => format!(
            "linear-gradient(135deg, color-mix(in oklch, {accent} 55%, black), \
             color-mix(in oklch, {accent} 28%, black))"
        ),
        _ => {
            let h = fallback_hue(name);
            format!("linear-gradient(135deg, oklch(0.38 0.05 {h}), oklch(0.22 0.035 {h}))")
        }
    }
}

/// Caption meta line: `"N books"` joined with `Smart`/`Public` markers, the
/// same composition as the iOS card meta.
pub(super) fn shelf_meta_line(count: i64, kind: ShelfKind, visibility: Visibility) -> String {
    let books = if count == 1 {
        "1 book".to_string()
    } else {
        format!("{count} books")
    };
    let mut parts = vec![books];
    if kind == ShelfKind::Smart {
        parts.push("Smart".to_string());
    }
    if visibility == Visibility::Public {
        parts.push("Public".to_string());
    }
    parts.join(" \u{00b7} ")
}

/// Per-render snapshot of the gallery's data + selection state, grouped to
/// stay under the 5-prop threshold — mirrors `landing::sections`'
/// `LandingContentProps`.
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

/// The gallery. Tiles are toggle buttons (`aria-pressed` marks the pick);
/// the create-shelf modal mounts locally, same wiring as the shelf rail.
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
    let all_plate = plate_gradient(None, "All Books");
    rsx! {
        section {
            class: "sg-gallery",
            "data-testid": "shelf-gallery",
            aria_label: "Shelves",
            div { class: "sg-head",
                span { class: "label", "Shelves" }
                button {
                    r#type: "button",
                    class: "shelf-rail-new",
                    "data-testid": "new-shelf",
                    onclick: move |_| show_create.set(true),
                    "\u{FF0B} New shelf"
                }
            }
            div { class: "sg-track",
                button {
                    r#type: "button",
                    class: if all_active { "sg-card sg-card--active" } else { "sg-card" },
                    "data-testid": "gallery-all-books",
                    aria_pressed: all_active,
                    onclick: move |_| on_select.call(ShelfSelection::All),
                    {mosaic(&all_cover_uuids, &server_url, &all_plate, Some(all_books_icon()))}
                    span { class: "sg-name", "All Books" }
                    span { class: "sg-meta mono", "{all_meta}" }
                }
                for s in shelves.iter() {
                    ShelfCardTile {
                        key: "{s.id}",
                        shelf: s.clone(),
                        active: selection == ShelfSelection::Shelf(s.id),
                        server_url: server_url.clone(),
                        on_select,
                    }
                }
                button {
                    r#type: "button",
                    class: "sg-new",
                    "data-testid": "gallery-new-shelf",
                    onclick: move |_| show_create.set(true),
                    span { class: "sg-new-plate", "\u{FF0B}" }
                    span { class: "sg-meta", "New shelf" }
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

/// One shelf tile: mosaic (or accent plate), kind badge, name, meta line.
#[component]
fn ShelfCardTile(
    shelf: ShelfSummary,
    active: bool,
    server_url: String,
    on_select: EventHandler<ShelfSelection>,
) -> Element {
    let id = shelf.id;
    let plate = plate_gradient(shelf.accent.as_deref(), &shelf.name);
    let meta = shelf_meta_line(shelf.book_count, shelf.kind, shelf.visibility);
    let badge = match shelf.kind {
        ShelfKind::Smart => Some(cog_icon()),
        ShelfKind::Wishlist => Some(heart_icon()),
        ShelfKind::Manual => None,
    };
    rsx! {
        button {
            r#type: "button",
            class: if active { "sg-card sg-card--active" } else { "sg-card" },
            "data-testid": "gallery-shelf-{id}",
            aria_pressed: active,
            onclick: move |_| on_select.call(ShelfSelection::Shelf(id)),
            span { class: "sg-mosaic-wrap",
                {mosaic(&shelf.cover_uuids, &server_url, &plate, badge.clone())}
                if let Some(glyph) = badge {
                    span { class: "sg-badge", {glyph} }
                }
            }
            span { class: "sg-name", "{shelf.name}" }
            span { class: "sg-meta mono", "{meta}" }
        }
    }
}

/// The 2:3 collage: cover cells per [`mosaic_layout`], or the accent plate
/// (with the shelf-kind glyph) when no member has a cover.
fn mosaic(
    cover_uuids: &[String],
    server_url: &str,
    plate: &str,
    glyph: Option<Element>,
) -> Element {
    let layout = mosaic_layout(cover_uuids.len());
    if layout == MosaicLayout::Empty {
        return rsx! {
            span { class: "sg-mosaic sg-mosaic--empty",
                span { class: "sg-plate", style: "background: {plate};",
                    if let Some(glyph) = glyph {
                        {glyph}
                    }
                }
            }
        };
    }
    let n = match layout {
        MosaicLayout::One => 1,
        MosaicLayout::Two => 2,
        MosaicLayout::Three => 3,
        _ => 4,
    };
    rsx! {
        span { class: "sg-mosaic sg-mosaic--{n}",
            for uuid in cover_uuids.iter().take(n) {
                img {
                    key: "{uuid}",
                    class: "sg-cell",
                    src: crate::thumb_url(server_url, uuid, "sm"),
                    alt: "",
                    loading: "lazy",
                    draggable: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
