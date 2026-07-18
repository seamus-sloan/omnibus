//! Hero section of the book-detail page — breadcrumb, cover with format badges, title row, CTAs, tag chips, rating card.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{BookFileInfo, EbookMetadata};

use crate::components::atrium::Cover;
use crate::Route;

use super::export_menu::BdExportMenu;
use super::file_picker::{is_audio_book_file, BdFilePickerMenu, FilePickerKind};
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
            BdCtaRow {
                uuid: uuid.clone(),
                has_ebook,
                has_audio,
                book_author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                book_title: title.clone(),
                epub_size_bytes: b.epub_size_bytes,
                book_files: b.book_files.clone(),
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
/// per-device send/download actions.
#[component]
fn BdCtaRow(
    uuid: String,
    has_ebook: bool,
    has_audio: bool,
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
    #[props(default)] epub_size_bytes: Option<i64>,
    #[props(default)] book_files: Vec<BookFileInfo>,
) -> Element {
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

/// The book+soundwave glyph on the Immersive Read CTA. Factored out so the
/// active (web) and disabled (mobile) buttons share identical markup.
fn bd_immersive_mark() -> Element {
    rsx! {
        span { class: "bd-immersive-mark", aria_hidden: "true",
            svg {
                width: "17",
                height: "17",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "1.7",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                path { d: "M4 5.2A2 2 0 0 1 6 4h4.2a1.8 1.8 0 0 1 1.8 1.8V18a1.6 1.6 0 0 0-1.6-1.6H6A2 2 0 0 1 4 14.4V5.2z" }
                path { d: "M15.5 8v8M18.5 6v12M21 9.5v5" }
            }
        }
    }
}

/// Immersive Read CTA (web): opens the reader and docks the audiobook player
/// together. Clicking retargets the app-wide [`crate::PlaybackState`] at this
/// book — the App-level audio bootstrap then loads its manifest and the `/read`
/// route's [`crate::pages::MiniDock`] surfaces (paused, at the resume position)
/// once book + uuid resolve — then navigates to the reader. Playback itself
/// starts on the first transport action, not on load. The playback context and
/// navigator are read inside the handler (not as render-time hooks) so the
/// button renders under SSR and in unit tests without a provider, keeping
/// hydration parity (rule 07).
#[cfg(not(feature = "mobile"))]
#[component]
fn BdImmersiveButton(uuid: String) -> Element {
    let on_click = move |_: MouseEvent| {
        let playback = consume_context::<crate::PlaybackState>();
        // Retarget only when the book differs, mirroring the listen page: clear
        // the previous book's metadata/error and flag loading first so the dock
        // can't flash the old book under the new reader before the driver reloads.
        let mut uuid_sig = playback.uuid;
        if uuid_sig.peek().as_deref() != Some(uuid.as_str()) {
            let mut book_sig = playback.book;
            let mut error_sig = playback.error;
            let mut loading_sig = playback.loading;
            book_sig.set(None);
            error_sig.set(None);
            loading_sig.set(true);
            uuid_sig.set(Some(uuid.clone()));
        }
        dioxus_router::navigator().push(Route::BookRead { uuid: uuid.clone() });
    };
    rsx! {
        button {
            class: "btn lg bd-immersive-cta",
            r#type: "button",
            "data-testid": "immersive-read",
            title: "Open the ereader and audiobook together, kept in sync",
            onclick: on_click,
            {bd_immersive_mark()}
            "Immersive Read"
        }
    }
}

/// Immersive Read CTA (mobile): disabled stub. Mobile has no web
/// [`crate::PlaybackState`] to dock a player into; the mobile immersive
/// experience is tracked separately (#1133).
#[cfg(feature = "mobile")]
#[component]
fn BdImmersiveButton(uuid: String) -> Element {
    let _ = uuid;
    rsx! {
        button {
            class: "btn lg bd-immersive-cta",
            disabled: true,
            "data-testid": "immersive-read",
            title: "Immersive reading on mobile coming soon",
            {bd_immersive_mark()}
            "Immersive Read"
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
                uuid: "book-uuid".to_string(),
                has_ebook,
                has_audio,
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
