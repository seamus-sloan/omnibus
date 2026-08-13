//! Body grid of the book-detail page — two-column main (public journal feed + saved passages + cover-fan rail) plus a sticky right rail (file details, [`FormatSwitcher`], series info, insights).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{
    BookInsights, BookSuggestion, EbookMetadata, Identifier, SuggestionsResponse,
};

use crate::components::atrium::Cover;
use crate::components::{BookActionMeta, FormatSwitcher};
use crate::{data, Route};

use super::dates::{fmt_long_date, local_date_offset, use_local_dates_ready};
use super::discovery::{
    cover_src, list_count_label, same_hand_author_label, same_hand_title, same_hand_year,
    suggestion_cover_book, SuggestionsSpinner,
};
use super::highlights::{BdHighlightsSection, BdQuoteMeta};
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

/// Main column: public journal feed, saved passages, from-the-same-hand fan,
/// and the "Readers also enjoyed" suggestions strip.
#[component]
pub(super) fn BdBodyMain(
    uuid: String,
    title: String,
    author: BdAuthorCluster,
    accent: Option<String>,
    suggestions: Option<SuggestionsResponse>,
    ctx: BdPageCtx,
) -> Element {
    rsx! {
        div { class: "bd-body-main",
            BdJournalSection { uuid: uuid.clone() }
            div { class: "divider" }
            BdHighlightsSection {
                uuid,
                quote_meta: BdQuoteMeta {
                    title: title.clone(),
                    author: author.primary_author.clone(),
                    accent,
                },
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

/// Sticky rail: file-details card, series/standalone card, reading-insights card.
#[component]
pub(super) fn BdRailSection(
    b: EbookMetadata,
    title: String,
    authors_line: String,
    series: Option<String>,
    merge_button: Option<Element>,
    delete_button: Option<Element>,
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
                                k: bd_identifier_label(ident),
                                v: ident.value.clone(),
                            }
                        }
                    }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Formats" }
                FormatSwitcher {
                    formats: b.formats.clone(),
                    meta: BookActionMeta {
                        uuid: uuid.clone(),
                        // Author + title drive the Send-to-Kobo `<Author>/<Title>/`
                        // folder layout on the device.
                        author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                        title: title.clone(),
                        epub_size_bytes: b.epub_size_bytes,
                        book_files: b.book_files.clone(),
                    },
                }
                Link {
                    to: Route::MetadataEdit { uuid: uuid.clone() },
                    class: "btn ghost sm bd-rail-edit",
                    "data-testid": "edit-metadata",
                    "Edit metadata\u{2026}"
                }
                {merge_button}
                {delete_button}
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
            BdInsightsCard { uuid: uuid.clone() }
        }
    }
}

/// Reading-insights card: Started / Time read / Sessions / Pace for this
/// book, plus the (still-static) Activity strip. The grid is seeded to the
/// em-dash placeholders on both SSR and the first WASM paint (rule 07) —
/// identical to a book with no sessions (AC2) — and a post-mount effect
/// fetches [`BookInsights`] from `rpc_book_insights`, re-rendering with
/// concrete values once it resolves. A book with no recorded sessions
/// resolves to `None` and the placeholders never change.
#[component]
fn BdInsightsCard(uuid: String) -> Element {
    let mut insights = use_signal(|| None::<BookInsights>);
    // Monotonic load counter, mirroring `BdReadStatusControl`/the rating
    // widget: a fast SPA navigation between two books can leave a slower
    // fetch for the *previous* uuid still in flight, and without this guard
    // its late response would overwrite the new book's insights.
    let mut load_seq = use_signal(|| 0u64);
    use_effect(use_reactive!(|uuid| {
        let my_load = *load_seq.peek() + 1;
        load_seq.set(my_load);
        insights.set(None);
        spawn(async move {
            if let Ok(fetched) = data::get_book_insights(&uuid).await {
                if *load_seq.peek() == my_load {
                    insights.set(fetched);
                }
            }
        });
    }));
    // Hoisted once (rule 07): SSR and first paint render the UTC day, the
    // post-mount flip re-renders the Started cell in the viewer's local day.
    let dates_ready = use_local_dates_ready();
    let [started, time_read, sessions, pace] = bd_insight_values(insights(), dates_ready());

    rsx! {
        div { class: "card", "data-testid": "insights-card",
            div { class: "bd-insights-head",
                div { class: "label", "Insights" }
                span { class: "mono bd-insights-tag", "this book" }
            }
            div { class: "bd-insights-grid",
                BdInsightCell { label: "Started".to_string(), value: started }
                BdInsightCell { label: "Time read".to_string(), value: time_read }
                BdInsightCell { label: "Sessions".to_string(), value: sessions }
                BdInsightCell { label: "Pace".to_string(), value: pace }
            }
            div { class: "divider" }
            div { class: "label bd-rail-head", "Activity · last 22 days" }
            div { class: "bd-activity-bar", aria_hidden: "true",
                for tick_idx in 0..22u32 { i { key: "{tick_idx}", class: "bd-activity-tick" } }
            }
            div { class: "bd-activity-axis mono",
                span { "3wk ago" }
                span { "minutes read · by day" }
                span { "today" }
            }
        }
    }
}

/// Started / Time read / Sessions / Pace cell values for the Insights grid.
/// `None` (no sessions yet, whether because the fetch hasn't resolved or the
/// book truly has none) renders all four as em-dashes — the SSR seed and the
/// AC2 empty state are the same value, so there is no flash-then-revert.
fn bd_insight_values(insights: Option<BookInsights>, dates_ready: bool) -> [String; 4] {
    let dash = || "\u{2014}".to_string();
    match insights {
        Some(i) if i.sessions > 0 => [
            fmt_long_date(i.started_at, local_date_offset(dates_ready, i.started_at)),
            bd_duration_label(i.seconds_total),
            i.sessions.to_string(),
            format!(
                "{}/session",
                bd_duration_label(i.seconds_total / i.sessions)
            ),
        ],
        _ => [dash(), dash(), dash(), dash()],
    }
}

/// "Xh Ym" / "Xm" duration label for the Time-read and Pace cells. Never
/// negative — `db::stats::book_insights` only ever sums non-negative session
/// durations, but the clamp keeps this fn safe to call standalone.
fn bd_duration_label(secs: i64) -> String {
    let secs = secs.max(0);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
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

/// Label for a file-details identifier row: a known scheme renders as-is; a missing or `unknown` scheme is inferred from the value's shape as `ISBN` or `Identifier`.
fn bd_identifier_label(ident: &Identifier) -> String {
    match ident.scheme.as_deref() {
        Some(scheme) if !scheme.eq_ignore_ascii_case("unknown") => scheme.to_string(),
        _ if bd_looks_like_isbn(&ident.value) => "ISBN".to_string(),
        _ => "Identifier".to_string(),
    }
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
