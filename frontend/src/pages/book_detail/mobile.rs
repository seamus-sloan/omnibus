//! Mobile layout for the book-detail page — re-flows the same loaded-book data
//! ([`super::LoadedBookView`]) into the native design's single-column surface:
//! an accent-tinted hero over About / rating / info / files / journal sections.
//! Reading stays stubbed on mobile (disabled CTA); listening opens the player.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{BookFileInfo, EbookMetadata};

use crate::components::atrium::Cover;
use crate::Route;

use super::file_picker::{is_audio_book_file, BdFilePickerMenu, FilePickerKind};
use super::journal::BdJournalSection;
use super::rating::BdRatingWidget;
use super::{derive_loaded_view, BdFormatBadge, BdMetaRow, LoadedBookView};

/// Render the fully-loaded book detail for the mobile shell. A plain fn (no
/// hooks) — [`super::render_loaded`] owns the page's hook sequence and calls
/// this to produce the body.
pub(super) fn render_loaded_mobile(b: EbookMetadata, server_url: String) -> Element {
    let LoadedBookView {
        title,
        authors_line,
        series,
        accent_style,
        has_audio,
        has_ebook,
        ..
    } = derive_loaded_view(&b);

    let uuid = b.unique_identifier.clone().unwrap_or_default();
    let year = b
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("")
        .to_string();
    let meta_line = match (authors_line.is_empty(), year.is_empty()) {
        (false, false) => format!("{authors_line} · {year}"),
        (false, true) => authors_line.clone(),
        (true, false) => year.clone(),
        (true, true) => String::new(),
    };
    let (cover_src, cover_srcset) = thumb_srcs(&b, &uuid, &server_url);
    let epub_files: Vec<BookFileInfo> = b
        .book_files
        .iter()
        .filter(|f| f.format.eq_ignore_ascii_case("EPUB"))
        .cloned()
        .collect();
    let audio_files: Vec<BookFileInfo> = b
        .book_files
        .iter()
        .filter(|f| is_audio_book_file(f))
        .cloned()
        .collect();

    rsx! {
        div { class: "m-bd", style: "{accent_style}", "data-testid": "mobile-book-detail",
            // Hero — accent glow, top actions, centered cover.
            div { class: "m-bd-hero",
                div { class: "m-bd-hero-bar",
                    // History-aware back; Landing is only the deep-link fallback.
                    button {
                        r#type: "button",
                        class: "m-icon-btn",
                        "aria-label": "Back",
                        "data-testid": "mobile-bd-back",
                        onclick: move |_| {
                            let nav = dioxus_router::navigator();
                            if nav.can_go_back() {
                                nav.go_back();
                            } else {
                                nav.replace(Route::Landing {});
                            }
                        },
                        "\u{2190}"
                    }
                    Link {
                        to: Route::MetadataEdit { uuid: uuid.clone() },
                        class: "m-icon-btn",
                        "aria-label": "Edit metadata",
                        "\u{22EF}"
                    }
                }
                div { class: "m-bd-cover",
                    Cover {
                        book: b.clone(),
                        src_override: cover_src,
                        srcset: cover_srcset,
                        sizes: Some("150px".to_string()),
                    }
                }
            }

            div { class: "m-bd-titlecol",
                h2 { class: "m-bd-title", span { class: "m-em", "{title}" } }
                if !meta_line.is_empty() {
                    div { class: "m-bd-meta", "{meta_line}" }
                }
                if !b.formats.is_empty() {
                    div { class: "m-bd-badges",
                        for f in b.formats.iter() {
                            BdFormatBadge { key: "{f}", fmt: f.clone() }
                        }
                    }
                }
            }

            // Primary CTAs — reading opens the in-app reader; listening opens
            // the player. A book with more than one file of the format a CTA
            // opens gets a picker alongside it (#1005); `BdFilePickerMenu`
            // renders nothing for a single-file book (AC2).
            div { class: "m-bd-cta",
                if has_ebook {
                    Link {
                        to: Route::BookRead { uuid: uuid.clone() },
                        class: "btn primary lg",
                        "Read"
                    }
                    BdFilePickerMenu { uuid: uuid.clone(), kind: FilePickerKind::Read, files: epub_files.clone() }
                }
                if has_audio {
                    Link {
                        to: Route::BookListen { uuid: uuid.clone() },
                        class: "btn lg",
                        "Listen"
                    }
                    BdFilePickerMenu { uuid: uuid.clone(), kind: FilePickerKind::Listen, files: audio_files.clone() }
                }
            }

            // About
            if b.description.as_deref().map(|d| !d.trim().is_empty()).unwrap_or(false) || !b.subjects.is_empty() {
                section { class: "m-section",
                    div { class: "label", "About" }
                    if let Some(desc) = b.description.as_deref() {
                        div { class: "m-bd-desc", dangerous_inner_html: "{desc}" }
                    }
                    if !b.subjects.is_empty() {
                        div { class: "m-bd-tags",
                            for tag in b.subjects.iter() {
                                span { key: "{tag}", class: "chip", "{tag}" }
                            }
                        }
                    }
                }
            }

            // Rating
            section { class: "m-section m-section-row",
                div { class: "label", "Your rating" }
                BdRatingWidget { uuid: uuid.clone() }
            }

            // Book info
            section { class: "m-section",
                div { class: "label", "Book info" }
                table { class: "bd-meta-table mono m-bd-info",
                    tbody {
                        if let Some(p) = b.publisher.clone() { BdMetaRow { k: "Publisher".to_string(), v: p } }
                        if let Some(d) = b.published.clone() { BdMetaRow { k: "Published".to_string(), v: d } }
                        if let Some(l) = b.language.clone() { BdMetaRow { k: "Language".to_string(), v: l } }
                        if let Some(a) = b.added_at.clone() { BdMetaRow { k: "Added".to_string(), v: a } }
                        if let Some(s) = series.clone() { BdMetaRow { k: "Series".to_string(), v: s } }
                        for (i, ident) in b.identifiers.iter().enumerate() {
                            BdMetaRow {
                                key: "{i}",
                                k: ident.scheme.clone().unwrap_or_else(|| "ID".into()),
                                v: ident.value.clone(),
                            }
                        }
                    }
                }
            }

            // Files
            section { class: "m-section",
                div { class: "label", "Files" }
                {files_list(&b)}
            }

            // Journal — reuse the shared reading-journal section.
            section { class: "m-section",
                BdJournalSection { uuid: uuid.clone() }
            }

            div { class: "m-bd-footer",
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}

/// File rows: per-file detail when the book carries `book_files`, otherwise
/// one row per format.
fn files_list(b: &EbookMetadata) -> Element {
    if !b.book_files.is_empty() {
        rsx! {
            div { class: "m-bd-files",
                for f in b.book_files.iter() {
                    div { key: "{f.id}", class: "m-bd-file",
                        BdFormatBadge { fmt: f.format.clone() }
                        div { class: "m-bd-file-body",
                            div { class: "m-bd-file-name", "{f.label.clone().unwrap_or_else(|| f.filename.clone())}" }
                            div { class: "mono m-bd-file-path", "{f.filename}" }
                        }
                    }
                }
            }
        }
    } else {
        rsx! {
            div { class: "m-bd-badges",
                for f in b.formats.iter() {
                    BdFormatBadge { key: "{f}", fmt: f.clone() }
                }
            }
        }
    }
}

/// Responsive thumbnail `src`/`srcset` for the hero cover.
fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
        // `thumb_url` appends the mobile `?token=` an `<img>` fetch needs (see `contexts::media_url`).
        let sm = crate::thumb_url(server_url, uuid, "sm");
        let md = crate::thumb_url(server_url, uuid, "md");
        let lg = crate::thumb_url(server_url, uuid, "lg");
        (
            Some(md.clone()),
            Some(format!("{sm} 160w, {md} 320w, {lg} 640w")),
        )
    } else {
        (None, None)
    }
}
