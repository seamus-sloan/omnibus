//! Hero section of the book-detail page — breadcrumb, cover with format badges, title row, CTAs, tag chips, rating card.

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
                div { class: "bd-desc", "data-testid": "book-description", dangerous_inner_html: "{description}" }
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

    rsx! {
        div { class: "bd-cta-row",
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
}
