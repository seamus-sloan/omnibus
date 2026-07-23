//! Mobile layout for the book-detail page — re-flows the same loaded-book data
//! ([`super::LoadedBookView`]) into the native design's single-column surface:
//! an accent-tinted hero over About / rating / info / files / journal sections.
//! Read opens the reader, Listen the player, and (for dual-format books)
//! Immersive Read opens the reader with the audiobook docked.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::summary::summary_is_sparse;
use omnibus_shared::{
    BookFileInfo, BookSuggestion, EbookMetadata, MetadataOverrides, SuggestionsResponse,
};

use crate::components::atrium::Cover;
use crate::components::FetchSummaryButton;
use crate::{data, Route};

use super::discovery::{
    cover_src, list_count_label, same_hand_author_label, same_hand_title, same_hand_year,
    suggestion_cover_book, SuggestionsSpinner,
};
use super::file_picker::{is_audio_book_file, BdFilePickerMenu, FilePickerKind};
use super::immersive::BdImmersiveButton;
use super::journal::BdJournalSection;
use super::rating::BdRatingWidget;
use super::{derive_loaded_view, BdFormatBadge, BdMetaRow, LoadedBookView};

/// The loaded-book data the mobile layout reflows into its single column,
/// including the discovery blocks (author cluster + Hardcover suggestions).
pub(super) struct MobileBookView {
    pub b: EbookMetadata,
    pub author_books: Vec<EbookMetadata>,
    pub suggestions: Option<SuggestionsResponse>,
    pub is_admin: bool,
    pub server_url: String,
}

/// Render the fully-loaded book detail for the mobile shell. A plain fn (no
/// hooks) — [`super::render_loaded`] owns the page's hook sequence and calls
/// this to produce the body.
pub(super) fn render_loaded_mobile(view: MobileBookView) -> Element {
    let MobileBookView {
        b,
        author_books,
        suggestions,
        is_admin,
        server_url,
    } = view;
    let LoadedBookView {
        title,
        primary_author,
        author_id,
        authors_line,
        series,
        accent_style,
        has_audio,
        has_ebook,
        ..
    } = derive_loaded_view(&b);

    let uuid = b.unique_identifier.clone().unwrap_or_default();

    // Local copy of the effective summary so a fetch-and-save can refresh the
    // shown description in place, mirroring web's `BdTitleCol`.
    let mut description = use_signal(|| b.description.clone().unwrap_or_default());
    let on_fetched = {
        let server_url = server_url.clone();
        let uuid = uuid.clone();
        move |text: String| {
            let url = server_url.clone();
            let uuid = uuid.clone();
            spawn(async move {
                let overrides = MetadataOverrides {
                    description: Some(text.clone()),
                    ..Default::default()
                };
                if data::save_overrides(&url, &uuid, &overrides).await.is_ok() {
                    description.set(text);
                }
            });
        }
    };

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

            // Multi-file actions open a picker; single-file actions navigate directly.
            div { class: "m-bd-cta",
                if has_ebook {
                    BdFilePickerMenu {
                        uuid: uuid.clone(),
                        kind: FilePickerKind::Read,
                        files: epub_files.clone(),
                        label: "Read",
                        button_class: "btn primary lg",
                        single_testid: "start-reading",
                    }
                }
                if has_audio {
                    BdFilePickerMenu {
                        uuid: uuid.clone(),
                        kind: FilePickerKind::Listen,
                        files: audio_files.clone(),
                        label: "Listen",
                        button_class: "btn lg",
                        single_testid: "start-listening",
                    }
                }
                // Immersive Read is a dual-format-only action, so gate it on both.
                if has_ebook && has_audio {
                    BdImmersiveButton { uuid: uuid.clone() }
                }
            }

            // About — always rendered: a sparse/missing summary (fewer than 10
            // words) still needs the section to host the Fetch Summary button.
            section { class: "m-section",
                div { class: "label", "About" }
                if !description().is_empty() {
                    div { class: "m-bd-desc", dangerous_inner_html: "{description()}" }
                }
                if !b.subjects.is_empty() {
                    div { class: "m-bd-tags",
                        for tag in b.subjects.iter() {
                            span { key: "{tag}", class: "chip", "{tag}" }
                        }
                    }
                }
                if summary_is_sparse(&description()) {
                    FetchSummaryButton { uuid: uuid.clone(), on_fetched }
                }
            }

            // Rating
            section { class: "m-section",
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

            // Offline downloads (Kindle-style local copies).
            super::offline::BdOfflineSection {
                uuid: uuid.clone(),
                title: title.clone(),
                has_ebook,
                has_audio,
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

            // Discovery — other books by the author, then Hardcover read-alikes.
            {same_hand_section(&primary_author, author_id, &author_books, &server_url)}
            {suggestions_section(&title, &suggestions, is_admin, &server_url)}

            div { class: "m-bd-footer",
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}

/// "From the same hand" — other books by the primary author as a horizontal
/// cover strip, or a short note when the library holds no others.
fn same_hand_section(
    primary_author: &str,
    author_id: Option<i64>,
    author_books: &[EbookMetadata],
    server_url: &str,
) -> Element {
    let author_route = author_id.map(|id| Route::AuthorDetail { id });
    // Only books with a real uuid can be linked; drop the rest so a tile never
    // emits a `/books/` route or an empty-uuid thumbnail URL.
    let linkable: Vec<&EbookMetadata> = author_books
        .iter()
        .filter(|ab| {
            ab.unique_identifier
                .as_deref()
                .is_some_and(|u| !u.is_empty())
        })
        .collect();
    rsx! {
        section { class: "m-section", "data-testid": "mobile-from-same-hand",
            div { class: "m-section-head",
                div { class: "label", "From the same hand" }
                if let Some(route) = author_route {
                    Link { to: route, class: "m-section-link", "Author \u{2192}" }
                }
            }
            if linkable.is_empty() {
                p { class: "m-strip-note",
                    "This is the only book by {same_hand_author_label(primary_author)} in your library so far."
                }
            } else {
                div { class: "m-strip",
                    for ab in linkable {
                        {same_hand_tile(ab, server_url)}
                    }
                }
            }
        }
    }
}

/// One cover tile in the same-hand strip — thumbnail (mobile `?token=`) linking
/// to the book, with title + optional year captions.
fn same_hand_tile(ab: &EbookMetadata, server_url: &str) -> Element {
    let uuid = ab.unique_identifier.clone().unwrap_or_default();
    let (src, srcset) = thumb_srcs(ab, &uuid, server_url);
    rsx! {
        Link {
            key: "{ab.id}",
            to: Route::BookDetail { uuid: uuid.clone() },
            class: "m-strip-cell",
            "data-testid": "mobile-same-hand-tile",
            Cover { book: ab.clone(), src_override: src, srcset, sizes: Some("30vw".to_string()) }
            div { class: "m-strip-title", "{same_hand_title(ab)}" }
            if let Some(year) = same_hand_year(ab) {
                div { class: "m-strip-sub", "{year}" }
            }
        }
    }
}

/// "Suggested for you" — Hardcover read-alikes as a horizontal strip. Mirrors
/// the web state machine: results, empty, not-configured (connect note), or a
/// quiet pending placeholder while the worker resolves.
fn suggestions_section(
    book_title: &str,
    suggestions: &Option<SuggestionsResponse>,
    is_admin: bool,
    server_url: &str,
) -> Element {
    rsx! {
        section { class: "m-section", "data-testid": "mobile-suggestions",
            div { class: "label", "Suggested for you" }
            match suggestions {
                Some(SuggestionsResponse::Ready { items }) if !items.is_empty() => rsx! {
                    div { class: "m-strip",
                        for s in items.iter().cloned() {
                            {suggestion_tile(s, server_url)}
                        }
                    }
                },
                Some(SuggestionsResponse::Ready { .. }) => rsx! {
                    p { class: "m-strip-note", "data-testid": "mobile-suggestions-empty",
                        "No read-alikes found for {book_title} yet."
                    }
                },
                Some(SuggestionsResponse::NotConfigured) => rsx! {
                    p { class: "m-strip-note", "data-testid": "mobile-suggestions-not-configured",
                        if is_admin {
                            "Add a Hardcover API key in Settings to see read-alikes."
                        } else {
                            "Ask your server admin to connect Hardcover for read-alikes."
                        }
                    }
                },
                // None (pre-fetch) or Pending → loading placeholder.
                _ => rsx! {
                    p { class: "m-strip-note m-suggest-loading", "data-testid": "mobile-suggestions-pending",
                        SuggestionsSpinner {}
                        "Looking for read-alikes via Hardcover\u{2026}"
                    }
                },
            }
        }
    }
}

/// One cover tile in the suggestions strip — links out to Hardcover in a new
/// tab, with the "on N lists" relevance label and title/author captions.
fn suggestion_tile(s: BookSuggestion, server_url: &str) -> Element {
    rsx! {
        a {
            key: "{s.hardcover_id}",
            class: "m-strip-cell",
            href: "{s.hardcover_url}",
            target: "_blank",
            rel: "noopener noreferrer",
            "data-testid": "mobile-suggestion-tile",
            div { class: "m-strip-cover",
                Cover { book: suggestion_cover_book(&s), src_override: cover_src(server_url, &s) }
                span { class: "m-strip-badge mono", "{list_count_label(s.list_count)}" }
            }
            div { class: "m-strip-title", "{s.title}" }
            div { class: "m-strip-sub", "{s.author}" }
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
