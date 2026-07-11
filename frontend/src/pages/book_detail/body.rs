//! Body grid of the book-detail page — two-column main (public journal feed + highlights stub + cover-fan rail) plus a sticky right rail (file details, [`FormatSwitcher`], series info, insights).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{BookSuggestion, Contributor, EbookMetadata, Identifier, SuggestionsResponse};

use crate::components::atrium::Cover;
use crate::components::FormatSwitcher;
use crate::Route;

use super::journal::BdJournalSection;
use super::{BdInsightCell, BdMetaRow, BdSectionHead};

/// Ambient page-level context for the body: server base URL and admin flag.
#[derive(Clone, PartialEq, Props)]
pub(super) struct BdPageCtx {
    pub server_url: String,
    pub is_admin: bool,
}

/// The "from the same hand" author cluster: name, id, and other books.
#[derive(Clone, PartialEq, Props)]
pub(super) struct BdAuthorCluster {
    pub primary_author: String,
    pub author_id: Option<i64>,
    pub author_books: Vec<EbookMetadata>,
}

/// Main column: public journal feed, highlights stub, from-the-same-hand fan,
/// and the F3.3 "Readers also enjoyed" suggestions strip.
#[component]
pub(super) fn BdBodyMain(
    uuid: String,
    title: String,
    author: BdAuthorCluster,
    suggestions: Option<SuggestionsResponse>,
    ctx: BdPageCtx,
) -> Element {
    rsx! {
        div { class: "bd-body-main",
            BdJournalSection { uuid }
            div { class: "divider" }
            BdSectionHead { kicker: "0 highlights".to_string(), title: "Passages you saved".to_string() }
            div { class: "bd-journal-empty card", aria_hidden: "true",
                p { class: "mono", "No highlights saved yet." }
                p { class: "bd-stub-hint", "Highlights land in F3.2." }
            }
            div { class: "divider" }
            BdSameHand { author }
            BdSuggestionsStrip {
                book_title: title,
                suggestions,
                ctx,
            }
        }
    }
}

/// "From the same hand" — an author-lead card heading a spreading cover stack
/// of up to four other books by the same author (Atrium `SameHandSection`).
/// Reuses the `.suggest-stack` / `.sug-cap` mechanics: the lead card sits first
/// and the covers fan out from behind it on hover. When the author has no other
/// books in the library, the lead card is paired with a short note instead.
#[component]
pub(super) fn BdSameHand(author: BdAuthorCluster) -> Element {
    let BdAuthorCluster {
        primary_author,
        author_id,
        author_books,
    } = author;
    let owned = author_books.len() + 1;
    let shown = author_books.len().min(4);
    let rest = author_books.len() - shown;
    let kicker = if primary_author.is_empty() {
        "More to read".to_string()
    } else {
        format!("More by {primary_author}")
    };
    let author_label = same_hand_author_label(&primary_author);
    let author_route = author_id.map(|id| Route::AuthorDetail { id });
    let action = author_route.clone().map(|route| {
        rsx! {
            Link { to: route, class: "btn ghost sm", "Author page \u{2192}" }
        }
    });

    rsx! {
        BdSectionHead { kicker, title: "From the same hand".to_string(), action }
        if author_books.is_empty() {
            div { class: "bd-same-hand-empty", "data-testid": "from-same-hand-empty",
                AuthorLead { author: primary_author.clone(), owned, rest, author_route: author_route.clone() }
                div { class: "bd-same-hand-note",
                    "This is the only book by {author_label} in your library so far."
                }
            }
        } else {
            p { class: "mono bd-same-hand-hint",
                "Hover the row to spread \u{00b7} click a cover to open the book, the portrait to open the author"
            }
            div { class: "suggest-stack", "data-testid": "from-same-hand",
                AuthorLead { author: primary_author.clone(), owned, rest, author_route: author_route.clone() }
                for ab in author_books.iter().take(4) {
                    Link {
                        key: "{ab.id}",
                        to: Route::BookDetail { uuid: ab.unique_identifier.clone().unwrap_or_default() },
                        class: "cover-link",
                        "data-testid": "from-same-hand-tile",
                        title: "Open {same_hand_title(ab)}",
                        Cover { book: ab.clone() }
                        div { class: "sug-cap",
                            div { class: "sh-title", "{same_hand_title(ab)}" }
                            if let Some(year) = same_hand_year(ab) {
                                div { class: "sh-sub", "{year}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Author "lead" card that heads the same-hand stack: portrait glyph, name, and
/// owned-count, with an optional "+N more" footer. When `author_route` is
/// `Some`, the portrait/name/footer are author-page links; otherwise they render
/// as plain text (a book whose author carries no resolvable id).
#[component]
fn AuthorLead(author: String, owned: usize, rest: usize, author_route: Option<Route>) -> Element {
    let initial = author
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let books_word = if owned == 1 { "book" } else { "books" };
    // Guard the tooltip against a missing author name so it never renders the
    // malformed "Open 's author page".
    let portrait_title = if author.trim().is_empty() {
        "Open author page".to_string()
    } else {
        format!("Open {author}'s author page")
    };
    rsx! {
        div { class: "cover-link author-lead",
            div { class: "author-lead-card",
                if let Some(route) = author_route.clone() {
                    Link {
                        to: route.clone(),
                        class: "author-portrait",
                        title: "{portrait_title}",
                        span { class: "glyph", "{initial}" }
                        span { class: "hint", "Author page \u{2192}" }
                    }
                    div {
                        Link { to: route.clone(), class: "author-lead-name", "{author}" }
                        div { class: "author-lead-count", "{owned} {books_word} in your library" }
                    }
                    if rest > 0 {
                        Link { to: route.clone(), class: "author-lead-more", "+{rest} more \u{2192}" }
                    }
                } else {
                    div { class: "author-portrait",
                        span { class: "glyph", "{initial}" }
                    }
                    div {
                        div { class: "author-lead-name", "{author}" }
                        div { class: "author-lead-count", "{owned} {books_word} in your library" }
                    }
                }
            }
        }
    }
}

/// Display title for a same-hand tile — the book title, falling back to the
/// on-disk filename (mirrors the shared `Cover` plate fallback).
fn same_hand_title(b: &EbookMetadata) -> String {
    match b.title.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => b.filename.clone(),
    }
}

/// Author surname for the empty-state note, falling back to a generic label
/// when the author name is unknown so the sentence stays well-formed.
fn same_hand_author_label(primary_author: &str) -> String {
    match primary_author.split_whitespace().last() {
        Some(last) => last.to_string(),
        None => "this author".to_string(),
    }
}

/// Four-digit publication year for the tile subline, or `None` when absent.
fn same_hand_year(b: &EbookMetadata) -> Option<String> {
    b.published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .filter(|y| !y.is_empty())
        .map(str::to_string)
}

/// "Readers also enjoyed" — Hardcover read-alikes below the metadata. Renders
/// its own divider + section head, then one of: a connect message (no key), a
/// quiet placeholder (resolving), the cover strip (results), or an empty note.
/// The section is always present (stable `suggestions-strip` testid); only its
/// inner content varies by state.
#[component]
pub(super) fn BdSuggestionsStrip(
    book_title: String,
    suggestions: Option<SuggestionsResponse>,
    ctx: BdPageCtx,
) -> Element {
    let BdPageCtx {
        server_url,
        is_admin,
    } = ctx;
    rsx! {
        div { class: "divider" }
        BdSectionHead {
            kicker: format!("If you liked {book_title}\u{2026}"),
            title: "Suggested for you".to_string(),
        }
        div { class: "bd-suggest", "data-testid": "suggestions-strip",
            match suggestions {
                Some(SuggestionsResponse::Ready { items }) if !items.is_empty() => rsx! {
                    BdSuggestionsList { items, server_url }
                },
                Some(SuggestionsResponse::Ready { .. }) => rsx! {
                    div { class: "bd-suggest-pending card", "data-testid": "suggestions-empty",
                        p { class: "mono bd-stub-hint", "No read-alikes found for this book yet." }
                    }
                },
                Some(SuggestionsResponse::NotConfigured) => rsx! {
                    SuggestionsConnectCard { is_admin }
                },
                // None (first paint, pre-fetch) or Pending → quiet placeholder.
                _ => rsx! {
                    div { class: "bd-suggest-pending card", "data-testid": "suggestions-pending",
                        p { class: "mono bd-stub-hint", "Looking for read-alikes via Hardcover\u{2026}" }
                    }
                },
            }
        }
    }
}

/// The cover strip itself — 5 covers stacked (hover to spread); a "show more"
/// reveals the rest into a flat grid. Each cover links out to Hardcover.
#[component]
fn BdSuggestionsList(items: Vec<BookSuggestion>, server_url: String) -> Element {
    let mut expanded = use_signal(|| false);
    let total = items.len();
    let collapsed_len = total.min(5);
    let show_more = total > collapsed_len;
    let visible = if expanded() { total } else { collapsed_len };
    let row_class = if expanded() {
        "suggest-static"
    } else {
        "suggest-stack"
    };

    rsx! {
        div { class: "{row_class}",
            for s in items.iter().take(visible).cloned() {
                a {
                    key: "{s.hardcover_id}",
                    class: "cover-link suggest-card",
                    href: "{s.hardcover_url}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "data-testid": "suggestion-card",
                    Cover {
                        book: suggestion_cover_book(&s),
                        src_override: cover_src(&server_url, &s),
                    }
                    div { class: "fan-match mono", "{list_count_label(s.list_count)}" }
                    div { class: "sug-cap",
                        div { class: "sug-cap-title", "{s.title}" }
                        div { class: "sug-cap-author", "{s.author}" }
                    }
                }
            }
        }
        if show_more && !expanded() {
            div { class: "bd-suggest-actions",
                button {
                    class: "btn sm",
                    "data-testid": "suggestions-show-more",
                    onclick: move |_| expanded.set(true),
                    "Show {total - collapsed_len} more"
                }
            }
        }
    }
}

/// "Connect Hardcover" message shown when no API key is configured. Admins get
/// a link to Settings; everyone else is told to ask their server admin.
#[component]
fn SuggestionsConnectCard(is_admin: bool) -> Element {
    rsx! {
        div { class: "bd-suggest-connect card", "data-testid": "suggestions-not-configured",
            div {
                h4 { class: "bd-suggest-connect-title", "Suggestions are powered by Hardcover" }
                p { class: "bd-suggest-connect-body",
                    "Add a Hardcover API key to pull read-alikes for this book."
                }
            }
            if is_admin {
                Link {
                    to: Route::Settings {},
                    class: "btn primary sm",
                    "data-testid": "suggestions-connect-link",
                    "Add Hardcover API key \u{2192}"
                }
            } else {
                span { class: "mono bd-stub-hint", "Ask your server admin to connect Hardcover." }
            }
        }
    }
}

/// Build a minimal [`EbookMetadata`] so the shared [`Cover`] can render a
/// suggestion's title/author plate when no cover image is available.
fn suggestion_cover_book(s: &BookSuggestion) -> EbookMetadata {
    EbookMetadata {
        title: Some(s.title.clone()),
        creators: vec![Contributor {
            name: s.author.clone(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Absolute cover URL (server base + relative path), or `None` when the
/// suggestion has no cached cover (the plate fallback then applies).
fn cover_src(server_url: &str, s: &BookSuggestion) -> Option<String> {
    s.cover_url
        .as_deref()
        .map(|path| crate::media_url(server_url, path))
}

/// "on N lists" relevance label (singular-aware).
fn list_count_label(n: i64) -> String {
    if n == 1 {
        "on 1 list".to_string()
    } else {
        format!("on {n} lists")
    }
}

/// Sticky rail: file-details card, series/standalone card, reading-insights card.
#[component]
pub(super) fn BdRailSection(
    b: EbookMetadata,
    title: String,
    authors_line: String,
    series: Option<String>,
    merge_button: Option<Element>,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    rsx! {
        aside { class: "bd-rail",
            div { class: "card",
                div { class: "label bd-rail-head", "File details" }
                table { class: "bd-meta-table mono",
                    tbody {
                        BdMetaRow { k: "Title".to_string(), v: title.clone() }
                        if !authors_line.is_empty() {
                            BdMetaRow { k: "Author".to_string(), v: authors_line.clone() }
                        }
                        if let Some(p) = b.publisher.clone() { BdMetaRow { k: "Pub.".to_string(), v: p } }
                        if let Some(d) = b.published.clone() { BdMetaRow { k: "Date".to_string(), v: d } }
                        if let Some(l) = b.language.clone() { BdMetaRow { k: "Language".to_string(), v: l } }
                        for ident in b.identifiers.iter() {
                            BdMetaRow {
                                key: "{bd_identifier_key(ident)}",
                                k: ident.scheme.clone().unwrap_or_else(|| "ID".into()),
                                v: ident.value.clone(),
                            }
                        }
                    }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Formats" }
                FormatSwitcher {
                    formats: b.formats.clone(),
                    uuid: uuid.clone(),
                    book_files: b.book_files.clone(),
                    // Author + title drive the Send-to-Kobo `<Author>/<Title>/`
                    // folder layout on the device.
                    book_author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                    book_title: title.clone(),
                    epub_size_bytes: b.epub_size_bytes,
                }
                Link {
                    to: Route::MetadataEdit { uuid: uuid.clone() },
                    class: "btn ghost sm bd-rail-edit",
                    "data-testid": "edit-metadata",
                    "Edit metadata\u{2026}"
                }
                {merge_button}
            }
            div { class: "card",
                if let Some(s) = series.as_ref() {
                    div { class: "label bd-rail-head", "Series" }
                    if let Some(sid) = b.series_id {
                        Link { to: Route::SeriesDetail { id: sid }, class: "bd-rail-body bd-series-link", "{s}" }
                    } else {
                        p { class: "bd-rail-body", "{s}" }
                    }
                } else {
                    div { class: "label bd-rail-head", "Standalone" }
                    p { class: "bd-rail-body", "Not part of a series." }
                }
            }
            div { class: "card",
                div { class: "bd-insights-head",
                    div { class: "label", "Insights" }
                    span { class: "mono bd-insights-tag", "this book" }
                }
                div { class: "bd-insights-grid", aria_hidden: "true",
                    BdInsightCell { label: "Started".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Time read".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Sessions".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Pace".to_string(), value: "—".to_string() }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Activity · last 22 days" }
                div { class: "bd-activity-bar", aria_hidden: "true",
                    for _ in 0..22u32 { i { class: "bd-activity-tick" } }
                }
                div { class: "bd-activity-axis mono",
                    span { "3wk ago" }
                    span { "minutes read · by day" }
                    span { "today" }
                }
            }
        }
    }
}

/// Collision-free list key for an identifier row. A book can carry several
/// identifiers sharing one `scheme` (the projection keeps every distinct
/// value per scheme), so the key folds in `value` to stay unique among the
/// keyed siblings — Dioxus panics when two keyed siblings share a key.
///
/// Both fields are `Debug`-quoted (not joined with a plain separator): a raw
/// `scheme|value` join collides when the data itself contains the delimiter
/// (`scheme="a|b", value="c"` vs `scheme="a", value="b|c"`), which would
/// reintroduce the very panic this guards against. `Debug` escapes embedded
/// quotes/backslashes, so the `(scheme, value)` pair maps injectively to the
/// key.
fn bd_identifier_key(ident: &Identifier) -> String {
    format!("{:?}\u{1f}{:?}", ident.scheme, ident.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bd_identifier_key_is_unique_for_same_scheme_distinct_values() {
        // The book-detail crash repro: two `unknown`-scheme identifiers on
        // one book must not collide on the rendered list key.
        let isbn = Identifier {
            value: "978-1-938570-40-7".into(),
            scheme: Some("unknown".into()),
        };
        let urn = Identifier {
            value: "urn:uuid:c0e51a66-085f-4805-b116-a0d451d281bd".into(),
            scheme: Some("unknown".into()),
        };
        assert_ne!(bd_identifier_key(&isbn), bd_identifier_key(&urn));
    }

    #[test]
    fn bd_identifier_key_is_unique_for_schemeless_distinct_values() {
        let a = Identifier {
            value: "a".into(),
            scheme: None,
        };
        let b = Identifier {
            value: "b".into(),
            scheme: None,
        };
        assert_ne!(bd_identifier_key(&a), bd_identifier_key(&b));
    }

    #[test]
    fn bd_identifier_key_does_not_collide_when_data_contains_the_delimiter() {
        // A naive `scheme|value` join would map both of these to "a|b|c";
        // the `Debug`-quoted encoding keeps them distinct.
        let split_scheme = Identifier {
            value: "c".into(),
            scheme: Some("a|b".into()),
        };
        let split_value = Identifier {
            value: "b|c".into(),
            scheme: Some("a".into()),
        };
        assert_ne!(
            bd_identifier_key(&split_scheme),
            bd_identifier_key(&split_value)
        );
    }

    #[test]
    fn same_hand_title_prefers_title_over_filename() {
        let b = EbookMetadata {
            title: Some("Piranesi".into()),
            filename: "piranesi.epub".into(),
            ..Default::default()
        };
        assert_eq!(same_hand_title(&b), "Piranesi");
    }

    #[test]
    fn same_hand_title_falls_back_to_filename_when_title_blank() {
        // `None`, empty, and whitespace-only titles all fall back so a tile
        // never renders a blank caption.
        for title in [None, Some("".into()), Some("   ".into())] {
            let b = EbookMetadata {
                title,
                filename: "dune.epub".into(),
                ..Default::default()
            };
            assert_eq!(same_hand_title(&b), "dune.epub");
        }
    }

    #[test]
    fn same_hand_year_extracts_four_digit_prefix() {
        let b = EbookMetadata {
            published: Some("2020-09-15".into()),
            ..Default::default()
        };
        assert_eq!(same_hand_year(&b), Some("2020".to_string()));
    }

    #[test]
    fn same_hand_year_is_none_when_missing_or_too_short() {
        let missing = EbookMetadata::default();
        assert_eq!(same_hand_year(&missing), None);
        let short = EbookMetadata {
            published: Some("20".into()),
            ..Default::default()
        };
        // `get(0..4)` returns `None` for a string shorter than four bytes.
        assert_eq!(same_hand_year(&short), None);
    }

    #[test]
    fn same_hand_author_label_uses_surname_when_present() {
        assert_eq!(same_hand_author_label("Ada Lovelace"), "Lovelace");
        assert_eq!(same_hand_author_label("Homer"), "Homer");
    }

    #[test]
    fn same_hand_author_label_falls_back_when_author_blank() {
        assert_eq!(same_hand_author_label(""), "this author");
        assert_eq!(same_hand_author_label("   "), "this author");
    }
}
