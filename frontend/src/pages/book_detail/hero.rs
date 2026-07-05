//! Hero section of the book-detail page — breadcrumb, cover with format badges, title row, CTAs, tag chips, rating card.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::Route;

use super::export_menu::BdExportMenu;
use super::rating::BdRatingWidget;
use super::{BdCrumb, BdCrumbItem, BdFormatBadge};

/// Which formats this book has, driving the hero's CTA buttons and format badges.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Availability {
    pub has_ebook: bool,
    pub has_audio: bool,
}

/// Hero section: breadcrumb, cover + format badges, title + CTAs, rating card.
#[component]
pub(super) fn BdHeroSection(
    b: EbookMetadata,
    title: String,
    kicker: String,
    crumbs: Vec<BdCrumbItem>,
    avail: Availability,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    rsx! {
        section { class: "bd-hero",
            BdCrumb { items: crumbs }
            div { class: "bd-hero-grid",
                div { class: "bd-cover-col",
                    Cover { book: b.clone() }
                    if !b.formats.is_empty() {
                        div { class: "bd-format-badges",
                            for f in b.formats.iter() {
                                BdFormatBadge { key: "{f}", fmt: f.clone() }
                            }
                        }
                    }
                }
                BdTitleCol {
                    b: b.clone(),
                    title: title.clone(),
                    kicker: kicker.clone(),
                    uuid: uuid.clone(),
                    avail,
                }
                aside { class: "card bd-rating-card",
                    div { class: "label", "Your rating" }
                    BdRatingWidget { uuid: uuid.clone() }
                    div { class: "divider" }
                    div { class: "label bd-action-head", "Actions" }
                    div { class: "bd-actions",
                        a { class: "btn ghost bd-action-row", href: "#journal",
                            "data-testid": "hero-write-journal",
                            span { "Write a journal entry" }
                            span { class: "bd-action-row-arrow", "\u{2192}" }
                        }
                        button { class: "btn ghost bd-action-row", disabled: true,
                            span { "Add a highlight" }
                            span { class: "bd-action-row-arrow", "\u{2192}" }
                        }
                        button { class: "btn ghost bd-action-row", disabled: true,
                            span { "Mark as finished" }
                            span { class: "bd-action-row-arrow", "\u{2192}" }
                        }
                        button { class: "btn ghost bd-action-row", disabled: true,
                            span { "Share or export\u{2026}" }
                            span { class: "bd-action-row-arrow", "\u{2192}" }
                        }
                    }
                    div { class: "divider" }
                    div { class: "label bd-shelves-head", "On your shelves" }
                    div { class: "bd-shelves", aria_hidden: "true",
                        div { class: "chip", "Not on a shelf" }
                    }
                }
            }
        }
    }
}

/// Title + CTAs column inside the hero grid.
#[component]
fn BdTitleCol(
    b: EbookMetadata,
    title: String,
    kicker: String,
    uuid: String,
    avail: Availability,
) -> Element {
    let Availability {
        has_ebook,
        has_audio,
    } = avail;
    rsx! {
        div { class: "bd-title-col",
            div { class: "label", "{kicker}" }
            div { class: "bd-title-row",
                h1 { class: "bd-title", "{title}" }
                Link {
                    to: Route::MetadataEdit { uuid: uuid.clone() },
                    class: "btn ghost sm bd-edit-hero",
                    "data-testid": "edit-metadata-hero",
                    title: "Edit metadata\u{2026}",
                    "aria-label": "Edit metadata",
                    span { class: "bd-ico-pencil" }
                    "Edit"
                }
            }
            if !b.creators.is_empty() {
                p { class: "bd-by", "data-testid": "book-authors",
                    "by "
                    for (i, creator) in b.creators.iter().enumerate() {
                        if i > 0 { ", " }
                        if let Some(author_id) = creator.id {
                            Link {
                                key: "id-{author_id}",
                                to: Route::AuthorDetail { id: author_id },
                                class: "bd-author-link",
                                "{creator.name}"
                            }
                        } else {
                            span {
                                key: "name-{creator.name}-{creator.role:?}-{creator.file_as:?}",
                                class: "bd-author-link",
                                "{creator.name}"
                            }
                        }
                    }
                }
            }
            if let Some(desc) = b.description.as_deref() {
                div { class: "bd-desc", "data-testid": "book-description", dangerous_inner_html: "{desc}" }
            }
            BdCtaRow { uuid: uuid.clone(), has_ebook, has_audio }
            div { class: "bd-progress-meta", aria_hidden: "true",
                div { class: "bd-progress-line",
                    span { class: "mono", "Not started" }
                    span { class: "mono", "0%" }
                }
                div { class: "pbar", i { style: "width: 0%;" } }
            }
            if !b.subjects.is_empty() {
                ul { class: "bd-tag-list",
                    for tag in b.subjects.iter() {
                        li { key: "{tag}", class: "chip", "{tag}" }
                    }
                }
            }
        }
    }
}

/// CTA button row: primary read/listen action, secondary listen, and the
/// "Export" dropdown that collects the per-device send/download actions.
#[component]
fn BdCtaRow(uuid: String, has_ebook: bool, has_audio: bool) -> Element {
    rsx! {
        div { class: "bd-cta-row",
            if has_ebook {
                {
                    #[cfg(not(feature = "mobile"))]
                    let start_reading = rsx! {
                        Link { to: Route::BookRead { uuid: uuid.clone() }, class: "btn primary lg", "data-testid": "start-reading", "Start reading" }
                    };
                    #[cfg(feature = "mobile")]
                    let start_reading = rsx! {
                        button { class: "btn primary lg", disabled: true, title: "Reading on mobile coming soon", "Start reading" }
                    };
                    start_reading
                }
            } else if has_audio {
                {
                    #[cfg(not(feature = "mobile"))]
                    let start_listening = rsx! {
                        Link { to: Route::BookListen { uuid: uuid.clone() }, class: "btn primary lg", "data-testid": "start-listening", "Start listening" }
                    };
                    #[cfg(feature = "mobile")]
                    let start_listening = rsx! {
                        button { class: "btn primary lg", disabled: true, title: "Listening on mobile coming soon", "Start listening" }
                    };
                    start_listening
                }
            }
            if has_audio && has_ebook {
                {
                    #[cfg(not(feature = "mobile"))]
                    let listen_btn = rsx! {
                        Link { to: Route::BookListen { uuid: uuid.clone() }, class: "btn lg", "data-testid": "listen-secondary", "Listen" }
                    };
                    #[cfg(feature = "mobile")]
                    let listen_btn = rsx! {
                        button { class: "btn lg", disabled: true, title: "Listening on mobile coming soon", "Listen" }
                    };
                    listen_btn
                }
            }
            // Download + Send-to-Kindle/Kobo live behind one "Export" menu so
            // the CTA row stays a single primary action plus this dropdown.
            BdExportMenu { uuid: uuid.clone(), has_ebook, has_audio }
        }
    }
}
