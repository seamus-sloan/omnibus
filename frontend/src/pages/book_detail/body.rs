//! Shared discovery sections of the book-detail page — "From the same hand"
//! (author lead + cover stack) and the Hardcover suggestions strip — plus the
//! identifier-label helpers the files stop's metadata table reuses. The marquee
//! stops compose these from `marquee.rs`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{BookSuggestion, EbookMetadata, Identifier, SuggestionsResponse};

use crate::components::atrium::Cover;
use crate::Route;

use super::discovery::{
    cover_src, list_count_label, same_hand_author_label, same_hand_title, same_hand_year,
    suggestion_cover_book, SuggestionsSpinner,
};
use super::BdSectionHead;

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
    // Only books with a real uuid can be linked; drop the rest so a tile never
    // emits a `/books/` route or an empty-uuid thumbnail URL.
    let author_books: Vec<EbookMetadata> = author_books
        .into_iter()
        .filter(|ab| {
            ab.unique_identifier
                .as_deref()
                .is_some_and(|u| !u.is_empty())
        })
        .collect();
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
                // None (first paint, pre-fetch) or Pending → loading placeholder.
                _ => rsx! {
                    div { class: "bd-suggest-pending bd-suggest-loading card", "data-testid": "suggestions-pending",
                        SuggestionsSpinner {}
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
                    to: Route::Settings { section: Some("metadata".into()) },
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
pub(super) fn bd_identifier_key(ident: &Identifier) -> String {
    format!("{:?}\u{1f}{:?}", ident.scheme, ident.value)
}

/// How confidently a row's label names what it holds. Two identifiers can
/// carry the same value under different schemes — an EPUB 3 package writes
/// its ISBN once as `<dc:identifier>` and again as an ONIX codelist-5
/// refinement — and [`bd_identifier_rows`] keeps the best-named of them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LabelRank {
    /// Inferred from the value's shape; the scheme said nothing usable.
    Inferred,
    /// The source's own scheme, passed through as written.
    RawScheme,
    /// A scheme this table recognizes and can name properly.
    Known,
}

/// Human label for a scheme this table recognizes. Covers the ONIX
/// codelist-5 numbers an EPUB 3 `identifier-type` refinement carries (a
/// reader should never be shown the bare code `15`) and the scheme names
/// Calibre and the retailers write.
///
/// `uuid` is deliberately not "UUID": the value is the *source file's* uuid,
/// which is not the book's own uuid — the one Omnibus keys everything on and
/// shows in the URL.
fn bd_known_scheme_label(scheme: &str) -> Option<&'static str> {
    let label = match scheme.trim().to_ascii_lowercase().as_str() {
        "01" | "proprietary" => "Proprietary ID",
        "02" | "isbn-10" | "isbn10" => "ISBN-10",
        "03" | "gtin-13" | "ean" | "ean-13" => "EAN-13",
        "04" | "upc" => "UPC",
        "05" | "ismn-10" => "ISMN-10",
        "06" | "doi" => "DOI",
        "13" | "lccn" => "LCCN",
        "14" | "gtin-14" => "GTIN-14",
        "15" | "isbn-13" | "isbn13" => "ISBN-13",
        "17" | "isbn-a" => "ISBN-A",
        "22" | "urn" => "URN",
        "23" | "oclc" => "OCLC",
        "24" | "url" | "uri" => "URL",
        "25" | "ismn-13" => "ISMN-13",
        "isbn" => "ISBN",
        "asin" | "amazon" | "mobi-asin" => "ASIN",
        "google" => "Google Books ID",
        "goodreads" => "Goodreads ID",
        "calibre" => "Calibre ID",
        "uuid" => "Source UUID",
        _ => return None,
    };
    Some(label)
}

/// Label for a file-details identifier row, plus how well the source named
/// it. A recognized scheme gets a human label; an unrecognized *named* scheme
/// passes through as written; a missing, `unknown`, or bare-numeric scheme
/// falls back to the value's own shape, so no row is ever labelled with a raw
/// codelist number.
fn bd_identifier_label_ranked(ident: &Identifier) -> (String, LabelRank) {
    match ident.scheme.as_deref().map(str::trim) {
        Some(scheme) if bd_known_scheme_label(scheme).is_some() => (
            bd_known_scheme_label(scheme).unwrap_or(scheme).to_string(),
            LabelRank::Known,
        ),
        Some(scheme)
            if !scheme.is_empty()
                && !scheme.eq_ignore_ascii_case("unknown")
                // An unrecognized all-digit scheme is a codelist value this
                // table doesn't know, not a name worth showing a reader.
                && !scheme.chars().all(|c| c.is_ascii_digit()) =>
        {
            (scheme.to_string(), LabelRank::RawScheme)
        }
        _ if bd_looks_like_isbn(&ident.value) => ("ISBN".to_string(), LabelRank::Inferred),
        _ => ("Identifier".to_string(), LabelRank::Inferred),
    }
}

/// One row of the book-detail identifier table.
pub(super) struct BdIdentifierRow {
    /// Dioxus list key, from the identifier the row's label came from.
    pub key: String,
    pub label: String,
    pub value: String,
}

/// The identifier rows to render, deduplicated by value.
///
/// A book routinely carries one identifier under several schemes — an EPUB 3
/// package repeats its ISBN as an ONIX refinement, and a merge folds two
/// editions' identifier sets together — which listed one value on as many
/// rows as it had schemes. Two identifiers with the same value *are* the same
/// identifier, so the rows collapse to one, keeping the best-named label
/// (which also subsumes the `(scheme, value)` dedup the DB's primary key
/// already gives us) and the first occurrence's position.
pub(super) fn bd_identifier_rows(identifiers: &[Identifier]) -> Vec<BdIdentifierRow> {
    let mut rows: Vec<(BdIdentifierRow, LabelRank)> = Vec::new();
    for ident in identifiers {
        let value = ident.value.trim();
        if value.is_empty() {
            continue;
        }
        let (label, rank) = bd_identifier_label_ranked(ident);
        let row = BdIdentifierRow {
            key: bd_identifier_key(ident),
            label,
            value: value.to_string(),
        };
        match rows
            .iter_mut()
            .find(|(existing, _)| existing.value.eq_ignore_ascii_case(value))
        {
            // Strictly better only: ties keep the first occurrence, so the
            // order the projection emits stays the order a reader sees.
            Some(slot) if rank > slot.1 => *slot = (row, rank),
            Some(_) => {}
            None => rows.push((row, rank)),
        }
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

/// True when `value`, with hyphens and whitespace stripped, is the right length for an ISBN-10 or ISBN-13, digits-only except a trailing ISBN-10 `X` check digit.
fn bd_looks_like_isbn(value: &str) -> bool {
    let cleaned: Vec<char> = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    match cleaned.len() {
        10 => {
            let last = cleaned.len() - 1;
            cleaned
                .iter()
                .enumerate()
                .all(|(i, c)| c.is_ascii_digit() || (i == last && c.eq_ignore_ascii_case(&'x')))
        }
        13 => cleaned.iter().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
