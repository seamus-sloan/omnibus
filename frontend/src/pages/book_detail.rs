//! Book detail page — Atrium "Cinematic" redesign.
//!
//! Composes Atrium primitives ([`crate::components::atrium::Cover`], buttons,
//! cards, chips, dividers) into the layout sketched in
//! `screens/book-detail.jsx#DetailA` from the Omnibus design canvas:
//!
//! * **Hero** — accent-tinted radial backdrop, 240-px cover with format
//!   badges, 76-px italic-serif title, author link, description, primary
//!   CTAs, in-progress chip bar, tag chips, and a right-side "your rating"
//!   action card.
//! * **Body** — two-column grid with journal entries, highlights, and two
//!   cover-fan rows on the left; a sticky rail on the right holding file
//!   details (with the relocated [`crate::components::FormatSwitcher`]),
//!   series/standalone info, and reading insights.
//!
//! Backend-supplied fields (title / authors / description / cover / formats
//! / subjects / identifiers / publisher / language / published) wire through
//! from `data::get_ebook`. Sections that don't yet have backend support
//! (rating, journal, highlights, more-by-author, suggestions, reading
//! activity, shelves) render placeholder content with `aria-hidden` and a
//! `TODO(F<n>.<m>)` line citing the future roadmap doc that will replace
//! the stub.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::MergeBooksResult;

use crate::components::atrium::Cover;
use crate::components::FormatSwitcher;
#[cfg(not(feature = "mobile"))]
use crate::components::MergeDialog;
use crate::{data, use_server_url, Route};

/// Book detail page shell: fetches metadata then hands off to `render_loaded`.
#[component]
pub fn BookDetailPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut author_books: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    // Bumped after a merge/undo so the effect below refetches the book
    // (signals read inside `use_effect` re-arm it).
    let refresh = use_signal(|| 0u32);

    // Admin gating for the "Merge with…" affordance. Web-only — mobile
    // renders no admin surfaces; the server-side `AdminUser` extractor
    // on `rpc_merge_books` is the actual security boundary.
    #[cfg(feature = "web")]
    let mut is_admin = use_signal(|| false);
    #[cfg(feature = "web")]
    use_effect(move || {
        spawn(async move {
            if let Ok(Some(user)) = data::current_user().await {
                is_admin.set(user.is_admin);
            }
        });
    });

    #[cfg(not(feature = "mobile"))]
    let mut merge_open = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let mut merge_result: Signal<Option<MergeBooksResult>> = use_signal(|| None);
    #[cfg(not(feature = "mobile"))]
    let mut undo_error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let _ = refresh();
        let url = url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            loading.set(true);
            author_books.set(Vec::new());
            match data::get_ebook(&url, &uuid).await {
                Ok(b) => {
                    let author_fetch = b.as_ref().map(|inner| {
                        (
                            inner.creators.first().and_then(|c| c.id),
                            inner.unique_identifier.clone(),
                        )
                    });
                    book.set(b);
                    error.set(None);
                    loading.set(false);
                    if let Some((Some(aid), current_uuid)) = author_fetch {
                        if let Ok(Some(ad)) = data::get_author(&url, aid).await {
                            let still_current =
                                book().as_ref().and_then(|b| b.unique_identifier.as_ref())
                                    == current_uuid.as_ref();
                            if still_current {
                                let others: Vec<EbookMetadata> = ad
                                    .books
                                    .into_iter()
                                    .filter(|ab| ab.unique_identifier != current_uuid)
                                    .collect();
                                author_books.set(others);
                            }
                        }
                    }
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        });
    }));

    if loading() {
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }
    let Some(b) = book() else {
        return rsx! {
            p { class: "subtitle", "Book not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    };

    #[cfg(feature = "web")]
    let is_admin_flag = is_admin();
    #[cfg(not(feature = "web"))]
    let is_admin_flag = false;
    #[cfg_attr(feature = "mobile", allow(unused_variables))]
    let page_title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    #[cfg_attr(feature = "mobile", allow(unused_variables))]
    let page_uuid = b.unique_identifier.clone().unwrap_or_default();

    // Rail "Merge with…" button (admin, web only) — threaded down as a
    // prebuilt Element so the rail component stays platform-agnostic.
    #[cfg(not(feature = "mobile"))]
    let merge_button: Option<Element> = is_admin_flag.then(|| {
        rsx! {
            button {
                class: "btn ghost sm bd-rail-edit",
                "data-testid": "merge-with",
                onclick: move |_| merge_open.set(true),
                "Merge with\u{2026}"
            }
        }
    });
    #[cfg(feature = "mobile")]
    let merge_button: Option<Element> = {
        let _ = is_admin_flag;
        None
    };

    #[cfg(not(feature = "mobile"))]
    let merge_ui: Option<Element> = {
        let mut refresh = refresh;
        let undo_url = server_url.clone();
        Some(rsx! {
            if merge_open() {
                MergeDialog {
                    target_uuid: page_uuid.clone(),
                    target_title: page_title.clone(),
                    on_merged: move |res: MergeBooksResult| {
                        merge_open.set(false);
                        undo_error.set(None);
                        merge_result.set(Some(res));
                        refresh.set(refresh() + 1);
                    },
                    on_close: move |_| merge_open.set(false),
                }
            }
            if let Some(res) = merge_result() {
                div { class: "bd-merge-toast card", role: "status",
                    span { "Books merged." }
                    button {
                        class: "btn ghost sm",
                        "data-testid": "merge-undo",
                        onclick: move |_| {
                            let url = undo_url.clone();
                            let mut refresh = refresh;
                            spawn(async move {
                                match data::undo_merge(&url, res.merge_log_id).await {
                                    Ok(_) => {
                                        merge_result.set(None);
                                        refresh.set(refresh() + 1);
                                    }
                                    Err(e) => undo_error.set(Some(e.to_string())),
                                }
                            });
                        },
                        "Undo"
                    }
                    button {
                        class: "btn ghost sm",
                        "data-testid": "merge-toast-dismiss",
                        aria_label: "Dismiss",
                        onclick: move |_| merge_result.set(None),
                        "\u{00d7}"
                    }
                    if let Some(e) = undo_error() {
                        p { role: "alert", class: "bd-merge-error", "{e}" }
                    }
                }
            }
        })
    };
    #[cfg(feature = "mobile")]
    let merge_ui: Option<Element> = None;

    let body = render_loaded(b, author_books(), merge_button);
    rsx! {
        {body}
        {merge_ui}
    }
}

// ---------------------------------------------------------------------------
// View — split out so the loaded-book case is the only thing rendered and
// the data-fetch shell stays small.
// ---------------------------------------------------------------------------

fn kicker_label(year: &str) -> String {
    if year.is_empty() {
        "Book".to_string()
    } else {
        format!("Book · {year}")
    }
}

fn series_label(series: Option<&str>, index: Option<&str>) -> Option<String> {
    match (series, index) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.to_string()),
        _ => None,
    }
}

/// Build the breadcrumb item list for a loaded book.
fn build_crumbs(
    b: &EbookMetadata,
    title: &str,
    primary_author: &str,
    series_label: &Option<String>,
) -> Vec<BdCrumbItem> {
    let mut crumbs = vec![BdCrumbItem {
        text: "Home".to_string(),
        target: Some(Route::Landing {}),
    }];
    if !primary_author.is_empty() {
        let author_route = b
            .creators
            .first()
            .and_then(|c| c.id)
            .map(|id| Route::AuthorDetail { id });
        crumbs.push(BdCrumbItem {
            text: primary_author.to_string(),
            target: author_route,
        });
    }
    if let Some(label) = series_label.clone() {
        let series_route = b.series_id.map(|id| Route::SeriesDetail { id });
        crumbs.push(BdCrumbItem {
            text: label,
            target: series_route,
        });
    }
    crumbs.push(BdCrumbItem {
        text: title.to_string(),
        target: None,
    });
    crumbs
}

/// Render the fully-loaded book detail view.
fn render_loaded(
    b: EbookMetadata,
    author_books: Vec<EbookMetadata>,
    merge_button: Option<Element>,
) -> Element {
    let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    let primary_author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let authors_line = b
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let year = b
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("")
        .to_string();
    let kicker = kicker_label(&year);
    let series = series_label(b.series.as_deref(), b.series_index.as_deref());
    let accent_style = b
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    let has_audio = b
        .formats
        .iter()
        .any(|f| f.eq_ignore_ascii_case("m4b") || f.eq_ignore_ascii_case("mp3"));
    let has_ebook = b
        .formats
        .iter()
        .any(|f| f.eq_ignore_ascii_case("epub") || f.eq_ignore_ascii_case("pdf"));

    let crumbs = build_crumbs(&b, &title, &primary_author, &series);

    rsx! {
        div { class: "bd-root", style: "{accent_style}",
            BdHeroSection {
                b: b.clone(),
                title: title.clone(),
                kicker: kicker.clone(),
                crumbs,
                has_ebook,
                has_audio,
            }
            section { class: "bd-body-grid",
                BdBodyMain {
                    title: title.clone(),
                    primary_author: primary_author.clone(),
                    author_books: author_books.clone(),
                }
                BdRailSection {
                    b: b.clone(),
                    title: title.clone(),
                    authors_line: authors_line.clone(),
                    series: series.clone(),
                    merge_button,
                }
            }
            div { class: "bd-footer",
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
            // Hidden slots preserved for the F1.4 contract — the hero
            // rating card and the cover-fan strips are the visible
            // surfaces; these stay attached so anything keying off the
            // slot testids still finds them.
            div {
                class: "ratings-slot",
                "data-testid": "ratings-slot",
                aria_label: "Ratings \u{2014} coming soon",
                hidden: true,
            }
            div {
                class: "suggestions-slot",
                "data-testid": "suggestions-slot",
                aria_label: "Suggestions \u{2014} coming soon",
                hidden: true,
            }
        }
    }
}

/// Hero section: breadcrumb, cover + format badges, title + CTAs, rating card.
#[component]
fn BdHeroSection(
    b: EbookMetadata,
    title: String,
    kicker: String,
    crumbs: Vec<BdCrumbItem>,
    has_ebook: bool,
    has_audio: bool,
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
                    has_ebook,
                    has_audio,
                }
                aside { class: "card bd-rating-card",
                    div { class: "label", "Your rating" }
                    div { class: "bd-stars", aria_hidden: "true", BdStars { value: 0.0 } }
                    div { class: "mono bd-rating-meta", "Not rated yet" }
                    div { class: "divider" }
                    div { class: "label bd-action-head", "Actions" }
                    div { class: "bd-actions",
                        button { class: "btn ghost bd-action-row", disabled: true,
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
    has_ebook: bool,
    has_audio: bool,
) -> Element {
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
                                key: "{i}-{creator.name}",
                                to: Route::AuthorDetail { id: author_id },
                                class: "bd-author-link",
                                "{creator.name}"
                            }
                        } else {
                            span {
                                key: "{i}-{creator.name}",
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

/// CTA button row: primary read/listen action, secondary listen, send-to-device stubs.
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
            button { class: "btn lg ghost", disabled: true, title: "Send-to-Kindle coming soon", "Send to Kindle" }
            button { class: "btn lg ghost", disabled: true, title: "Send-to-Kobo coming soon", "Send to Kobo" }
        }
    }
}

/// Main column: journal stub, highlights stub, from-the-same-hand fan, suggestions stub.
#[component]
fn BdBodyMain(title: String, primary_author: String, author_books: Vec<EbookMetadata>) -> Element {
    rsx! {
        div { class: "bd-body-main",
            BdSectionHead { kicker: "Your journal · 0 entries".to_string(), title: "What you've written".to_string() }
            div { class: "bd-journal-empty card", aria_hidden: "true",
                p { class: "mono", "No journal entries yet." }
                p { class: "bd-stub-hint", "Journaling lands in F3.2." }
            }
            div { class: "divider" }
            BdSectionHead { kicker: "0 highlights".to_string(), title: "Passages you saved".to_string() }
            div { class: "bd-journal-empty card", aria_hidden: "true",
                p { class: "mono", "No highlights saved yet." }
                p { class: "bd-stub-hint", "Highlights land in F3.2." }
            }
            div { class: "divider" }
            BdSectionHead {
                kicker: if primary_author.is_empty() { "More to read".to_string() } else { format!("More by {primary_author}") },
                title: "From the same hand".to_string(),
            }
            if author_books.is_empty() {
                div { class: "bd-author-books-empty card", "data-testid": "from-same-hand-empty",
                    p { class: "mono", "No other books by this author in your library." }
                }
            } else {
                div { class: "bd-author-books-row", "data-testid": "from-same-hand",
                    for ab in author_books.iter() {
                        Link {
                            key: "{ab.id}",
                            to: Route::BookDetail { uuid: ab.unique_identifier.clone().unwrap_or_default() },
                            class: "bd-author-book-tile",
                            "data-testid": "from-same-hand-tile",
                            Cover { book: ab.clone() }
                        }
                    }
                }
            }
            div { class: "divider" }
            BdSectionHead {
                kicker: format!("If you liked {title}\u{2026}"),
                title: "Suggested for you".to_string(),
            }
            div { class: "bd-stub-strip card", aria_hidden: "true",
                p { class: "bd-stub-hint mono", "Suggestions land in F3.3." }
            }
        }
    }
}

/// Sticky rail: file-details card, series/standalone card, reading-insights card.
#[component]
fn BdRailSection(
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
                                key: "{ident.scheme.as_deref().unwrap_or(ident.value.as_str())}",
                                k: ident.scheme.clone().unwrap_or_else(|| "ID".into()),
                                v: ident.value.clone(),
                            }
                        }
                    }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Formats" }
                FormatSwitcher { formats: b.formats.clone(), uuid: uuid.clone() }
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

// ---------------------------------------------------------------------------
// Page-local primitives. None of these introduce business logic — they're
// markup-only adapters so the page reads as a composition of named blocks
// rather than nested rsx.
// ---------------------------------------------------------------------------

/// One breadcrumb segment. When `target` is `Some`, the segment renders as a
/// router `Link`; otherwise it's a plain `<span>` for the current page or a
/// segment without a resolvable detail route.
#[derive(Clone, PartialEq, Props)]
pub struct BdCrumbItem {
    pub text: String,
    pub target: Option<Route>,
}

/// Atrium-styled breadcrumb. Segments with a `target` route render as Links,
/// otherwise as plain spans. The final segment is always rendered as the
/// "current page" span regardless of target.
#[component]
fn BdCrumb(items: Vec<BdCrumbItem>) -> Element {
    let last_idx = items.len().saturating_sub(1);
    rsx! {
        nav { class: "bd-crumb", "aria-label": "breadcrumb",
            for (i, item) in items.iter().cloned().enumerate() {
                if i > 0 {
                    span { class: "bd-crumb-sep", "\u{203a}" }
                }
                if i == last_idx {
                    span { class: "bd-crumb-curr", "{item.text}" }
                } else if let Some(route) = item.target.clone() {
                    Link {
                        to: route,
                        class: if i == 0 { "bd-crumb-home" } else { "bd-crumb-step" },
                        "{item.text}"
                    }
                } else {
                    span { class: "bd-crumb-step", "{item.text}" }
                }
            }
        }
    }
}

/// Body section heading row — kicker label + serif title. The kicker stacks
/// above the title (mirrors `screens/_shared.jsx#SectionHead`).
#[component]
fn BdSectionHead(kicker: String, title: String) -> Element {
    rsx! {
        div { class: "bd-section-head",
            div { class: "bd-section-head-text",
                if !kicker.is_empty() {
                    div { class: "label bd-section-kicker", "{kicker}" }
                }
                h3 { class: "bd-section-title", "{title}" }
            }
        }
    }
}

/// Small monospace format pill (matches `screens/_shared.jsx#FormatBadge`).
#[component]
fn BdFormatBadge(fmt: String) -> Element {
    rsx! {
        div { class: "bd-fmt-badge", "data-testid": "bd-format-badge", "{fmt}" }
    }
}

/// Read-only star display. Half-filled stars are rounded down to nearest
/// integer in the stub; the interactive widget will replace this later.
#[component]
fn BdStars(value: f32) -> Element {
    let full = value.floor().clamp(0.0, 5.0) as u32;
    rsx! {
        span { class: "bd-stars-row",
            for i in 0..5u32 {
                span {
                    class: if i < full { "bd-star bd-star-on" } else { "bd-star" },
                    "\u{2605}"
                }
            }
        }
    }
}

#[component]
fn BdMetaRow(k: String, v: String) -> Element {
    rsx! {
        tr { class: "bd-meta-row",
            td { class: "bd-meta-k", "{k}" }
            td { class: "bd-meta-v", "{v}" }
        }
    }
}

#[component]
fn BdInsightCell(label: String, value: String) -> Element {
    rsx! {
        div { class: "bd-insight-cell",
            div { class: "mono bd-insight-label", "{label}" }
            div { class: "bd-insight-value", "{value}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicker_label_renders_year_when_present() {
        assert_eq!(kicker_label("2021"), "Book · 2021");
    }
    #[test]
    fn kicker_label_falls_back_to_book_when_year_empty() {
        assert_eq!(kicker_label(""), "Book");
    }
    #[test]
    fn series_label_formats_name_and_index() {
        assert_eq!(
            series_label(Some("Dune"), Some("2")),
            Some("Dune #2".into())
        );
    }
    #[test]
    fn series_label_without_index_is_just_name() {
        assert_eq!(series_label(Some("Dune"), None), Some("Dune".into()));
    }
    #[test]
    fn series_label_absent_series_is_none() {
        assert_eq!(series_label(None, Some("2")), None);
        assert_eq!(series_label(None, None), None);
    }
}
