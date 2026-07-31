//! Hero section of the book-detail page — breadcrumb, cover with format badges
//! and tag chips, title row, CTAs, and the rating rail card (rating,
//! reading status, wishlist slot, shelves).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::summary::summary_is_sparse;
use omnibus_shared::{BookFileInfo, EbookMetadata, MetadataOverrides};

use crate::components::atrium::Cover;
use crate::components::{BookActionMeta, FetchSummaryButton};
use crate::{data, use_server_url, Route};

use super::export_menu::BdExportMenu;
use super::file_picker::{is_audio_book_file, BdFilePickerMenu, FilePickerKind};
use super::immersive::BdImmersiveButton;
use super::physical::{BdBookIdentity, BdWishlistRailSlot};
use super::rating::BdRatingWidget;
use super::read_status::BdReadStatusControl;
use super::{BdCrumb, BdCrumbItem, BdFormatBadge, PhysSignals};

/// Which formats this book has, driving the hero's CTA buttons and format badges.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Availability {
    pub has_ebook: bool,
    pub has_audio: bool,
}

/// Accent-colored "Physical" badge shown in the format-badge row when the book
/// has at least one checked-in physical copy.
#[component]
fn BdPhysBadge() -> Element {
    rsx! {
        span {
            class: "bd-fmt-badge bd-fmt-badge--physical",
            "data-testid": "format-badge-physical",
            title: "In your physical collection",
            "Physical"
        }
    }
}

/// "Physical Wishlist" badge shown in the format-badge row when the book is on
/// the caller's wishlist. Client-only by construction: the wishlist signal is
/// `None` on SSR and the first WASM paint, so it appears after the post-mount
/// load (rule 07 holds).
#[component]
fn BdWishlistBadge() -> Element {
    rsx! {
        span {
            class: "bd-fmt-badge bd-fmt-badge--physical",
            "data-testid": "format-badge-wishlist",
            title: "On your physical wishlist",
            "Physical Wishlist"
        }
    }
}

/// Hero section: breadcrumb, cover + format badges + tags, title + CTAs, and
/// the rating rail card (rating, reading status, wishlist slot, shelves).
#[component]
pub(super) fn BdHeroSection(
    b: EbookMetadata,
    title: String,
    kicker: String,
    crumbs: Vec<BdCrumbItem>,
    avail: Availability,
    phys: PhysSignals,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    let on_wishlist = phys.wishlist.read().is_some();
    rsx! {
        section { class: "bd-hero",
            BdCrumb { items: crumbs }
            div { class: "bd-hero-grid",
                div { class: "bd-cover-col",
                    Cover { book: b.clone() }
                    if !b.formats.is_empty() || b.has_physical || on_wishlist {
                        div { class: "bd-format-badges",
                            for f in b.formats.iter() {
                                BdFormatBadge { key: "{f}", fmt: f.clone() }
                            }
                            if b.has_physical {
                                BdPhysBadge {}
                            }
                            if on_wishlist {
                                BdWishlistBadge {}
                            }
                        }
                    }
                    if !b.subjects.is_empty() {
                        ul { class: "bd-tag-list",
                            for tag in b.subjects.iter() {
                                li { key: "{tag}", class: "chip", "{tag}" }
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
                    div { class: "label bd-readstatus-head", "Reading status" }
                    BdReadStatusControl { uuid: uuid.clone() }
                    BdWishlistRailSlot {
                        identity: BdBookIdentity {
                            uuid: uuid.clone(),
                            has_physical: b.has_physical,
                            isbn: b.isbn13.clone(),
                            title: title.clone(),
                            author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                        },
                        phys,
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

    // Local copy of the effective summary so a fetch-and-save can refresh the
    // shown description in place. Seeded identically on SSR and first WASM
    // paint (from the merged `b.description`) to keep hydration happy.
    let mut description = use_signal(|| b.description.clone().unwrap_or_default());

    // Fetch + save + refresh: the button hands us the fetched text; we persist
    // it as a description override, then update the shown summary on success.
    let server_url = use_server_url();
    let save_uuid = uuid.clone();
    let on_fetched = move |text: String| {
        let url = server_url.clone();
        let uuid = save_uuid.clone();
        spawn(async move {
            let overrides = MetadataOverrides {
                description: Some(text.clone()),
                ..Default::default()
            };
            if data::save_overrides(&url, &uuid, &overrides).await.is_ok() {
                description.set(text);
            }
        });
    };

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
            if !description().is_empty() {
                div { class: "bd-desc", "data-testid": "book-description", dangerous_inner_html: "{description()}" }
            }
            // Fetch Summary appears only when the current summary is sparse
            // (fewer than 10 words) — a nudge to enrich a thin blurb.
            if summary_is_sparse(&description()) {
                FetchSummaryButton { uuid: uuid.clone(), on_fetched }
            }
            BdCtaRow {
                has_ebook,
                has_audio,
                meta: BookActionMeta {
                    uuid: uuid.clone(),
                    author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                    title: title.clone(),
                    epub_size_bytes: b.epub_size_bytes,
                    book_files: b.book_files.clone(),
                },
            }
        }
    }
}

/// CTA button row: primary read/listen action, secondary listen, the
/// per-format file picker, and the "Export" dropdown that collects the
/// per-device send/download actions. Over the line cap by design — the body
/// is one declarative rsx! block whose branches mirror the CTA layout.
#[component]
fn BdCtaRow(has_ebook: bool, has_audio: bool, meta: BookActionMeta) -> Element {
    let BookActionMeta {
        uuid,
        author: book_author,
        title: book_title,
        epub_size_bytes,
        book_files,
    } = meta;
    let epub_files: Vec<BookFileInfo> = book_files
        .iter()
        .filter(|f| f.format.eq_ignore_ascii_case("EPUB"))
        .cloned()
        .collect();
    let audio_files: Vec<BookFileInfo> = book_files
        .iter()
        .filter(|f| is_audio_book_file(f))
        .cloned()
        .collect();

    // A fileless book (no ebook/audiobook files — physical-only or wishlist)
    // has nothing to read, listen to, or export, so the reading CTAs give way
    // to a disclaimer.
    let is_fileless = !has_ebook && !has_audio;

    rsx! {
        div { class: "bd-cta-row",
            if is_fileless {
                p { class: "bd-no-files", "data-testid": "no-files-disclaimer",
                    "No ebook or audiobook files in your library yet \u{2014} this title is tracked from your physical collection or wishlist."
                }
            } else {
            if has_ebook {
                BdFilePickerMenu {
                    uuid: uuid.clone(),
                    kind: FilePickerKind::Read,
                    files: epub_files.clone(),
                    label: "Start reading",
                    button_class: "btn primary lg",
                    single_testid: "start-reading",
                }
            } else if has_audio {
                {
                    #[cfg(not(feature = "mobile"))]
                    let start_listening = rsx! {
                        BdFilePickerMenu {
                            uuid: uuid.clone(),
                            kind: FilePickerKind::Listen,
                            files: audio_files.clone(),
                            label: "Start listening",
                            button_class: "btn primary lg",
                            single_testid: "start-listening",
                        }
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
                        BdFilePickerMenu {
                            uuid: uuid.clone(),
                            kind: FilePickerKind::Listen,
                            files: audio_files.clone(),
                            label: "Listen",
                            button_class: "btn lg",
                            single_testid: "listen-secondary",
                        }
                    };
                    #[cfg(feature = "mobile")]
                    let listen_btn = rsx! {
                        button { class: "btn lg", disabled: true, title: "Listening on mobile coming soon", "Listen" }
                    };
                    listen_btn
                }
            }
            // Immersive Read is a dual-format-only action, so gate it on both.
            if has_audio && has_ebook {
                BdImmersiveButton { uuid: uuid.clone() }
            }
            // Download + Send-to-Kindle/Kobo live behind one "Export" menu so
            // the CTA row stays a single primary action plus this dropdown.
            // Author + title feed the Send-to-Kobo `<Author>/<Title>/` layout.
            BdExportMenu {
                uuid: uuid.clone(),
                has_ebook,
                has_audio,
                book_author: book_author.clone(),
                book_title: book_title.clone(),
                epub_size_bytes,
            }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// SSR-render the CTA row for a book with the given format availability.
    fn render_cta_row(has_ebook: bool, has_audio: bool) -> String {
        dioxus::ssr::render_element(rsx! {
            BdCtaRow {
                has_ebook,
                has_audio,
                meta: BookActionMeta {
                    uuid: "book-uuid".to_string(),
                    ..Default::default()
                },
            }
        })
    }

    #[test]
    fn immersive_cta_renders_when_book_has_both_ebook_and_audio() {
        let html = render_cta_row(true, true);
        assert!(html.contains("data-testid=\"immersive-read\""));
        assert!(html.contains("Immersive Read"));
    }

    #[test]
    fn immersive_cta_absent_when_book_has_ebook_only() {
        let html = render_cta_row(true, false);
        assert!(!html.contains("data-testid=\"immersive-read\""));
    }

    #[test]
    fn immersive_cta_absent_when_book_has_audio_only() {
        let html = render_cta_row(false, true);
        assert!(!html.contains("data-testid=\"immersive-read\""));
    }

    #[test]
    fn fileless_book_shows_no_files_disclaimer_and_hides_reading_ctas() {
        let html = render_cta_row(false, false);
        assert!(html.contains("data-testid=\"no-files-disclaimer\""));
        // No reading CTAs and no export menu for a book with no files.
        assert!(!html.contains("data-testid=\"start-reading\""));
        assert!(!html.contains("data-testid=\"start-listening\""));
    }

    #[test]
    fn book_with_files_shows_no_disclaimer() {
        let html = render_cta_row(true, false);
        assert!(!html.contains("data-testid=\"no-files-disclaimer\""));
    }

    #[test]
    fn phys_badge_renders_its_testid_and_label() {
        let html = dioxus::ssr::render_element(rsx! { BdPhysBadge {} });
        assert!(html.contains("data-testid=\"format-badge-physical\""));
        assert!(html.contains("Physical"));
    }

    #[test]
    fn wishlist_badge_renders_its_testid_and_label() {
        let html = dioxus::ssr::render_element(rsx! { BdWishlistBadge {} });
        assert!(html.contains("data-testid=\"format-badge-wishlist\""));
        assert!(html.contains("Physical Wishlist"));
    }
}
