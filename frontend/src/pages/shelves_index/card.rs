//! One card on the web shelves index: the shelf's member covers standing on a
//! ledge lit in its accent, then its name, kind and visibility, and its book
//! count beside the owner's avatar. The whole card links to the shelf.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ShelfKind, ShelfSummary};

use super::filter::{book_count_label, kind_label, visibility_label};
use crate::components::shelf_glyphs::{kind_icon, visibility_icon};
use crate::components::user_avatar::UserAvatar;
use crate::Route;

/// How many books stand on a card's ledge.
const LEDGE_BOOKS: usize = 4;

/// One shelf card. `is_viewer` names the owner "You".
#[component]
pub fn ShelfCard(
    shelf: ShelfSummary,
    is_viewer: bool,
    server_url: String,
    avatar_bust: u32,
) -> Element {
    let id = shelf.id;
    let accent = shelf
        .accent
        .clone()
        .unwrap_or_else(|| "var(--accent)".into());
    let owner_label = if is_viewer {
        "You".to_string()
    } else {
        shelf.owner_username.clone()
    };
    rsx! {
        div { class: "shv-cell", role: "listitem",
            Link {
                to: Route::ShelfDetail { id },
                class: "shv-card",
                style: "--shelf-accent: {accent};",
                "data-testid": "shelf-card-{id}",
                {ledge(&shelf, &server_url)}
                div { class: "shv-card-body",
                    div { class: "shv-card-name", "{shelf.name}" }
                    div { class: "shv-card-facets",
                        span { class: "shv-facet shv-facet--kind",
                            {kind_icon(shelf.kind)}
                            "{kind_label(shelf.kind)}"
                        }
                        span { class: "shv-facet",
                            {visibility_icon(shelf.visibility)}
                            "{visibility_label(shelf.visibility)}"
                        }
                    }
                    div { class: "shv-card-foot",
                        span { class: "shv-card-count", "{book_count_label(shelf.book_count)}" }
                        span { class: "shv-card-owner", "data-testid": "shelf-card-owner-{id}",
                            // The name sits beside it, so the photo's alt would
                            // only repeat it inside the link's accessible name.
                            span { class: "shv-card-owner-av", aria_hidden: "true",
                                UserAvatar {
                                    user_id: shelf.owner_user_id,
                                    name: shelf.owner_username.clone(),
                                    has_avatar: shelf.owner_has_avatar,
                                    class: "shv-av",
                                    bust: avatar_bust,
                                }
                            }
                            span { class: "shv-card-owner-name", "{owner_label}" }
                        }
                    }
                }
            }
        }
    }
}

/// The covers standing on the ledge; blank plates for members with no cover;
/// an empty plate carrying the kind glyph when the shelf holds nothing.
fn ledge(shelf: &ShelfSummary, server_url: &str) -> Element {
    let blanks = if shelf.cover_uuids.is_empty() {
        usize::try_from(shelf.book_count)
            .unwrap_or(0)
            .min(LEDGE_BOOKS)
    } else {
        0
    };
    rsx! {
        div { class: "shv-ledge", aria_hidden: "true",
            for uuid in shelf.cover_uuids.iter().take(LEDGE_BOOKS) {
                span { key: "{uuid}", class: "shv-ledge-book",
                    img {
                        src: crate::thumb_url(server_url, uuid, "sm"),
                        alt: "",
                        loading: "lazy",
                        draggable: false,
                    }
                }
            }
            for i in 0..blanks {
                span { key: "blank-{i}", class: "shv-ledge-book shv-ledge-book--blank" }
            }
            if shelf.book_count == 0 {
                span { class: "shv-ledge-empty",
                    {kind_icon(shelf.kind)}
                    "{empty_ledge_label(shelf.kind)}"
                }
            }
        }
    }
}

/// What an empty ledge says, in the kind's own terms.
fn empty_ledge_label(kind: ShelfKind) -> &'static str {
    match kind {
        ShelfKind::Smart => "No matches yet",
        ShelfKind::Manual => "Empty shelf",
        ShelfKind::Wishlist => "Nothing wished for",
    }
}
