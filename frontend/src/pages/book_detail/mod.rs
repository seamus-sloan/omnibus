//! Book detail page — Atrium "Cinematic" shell.
//!
//! Owns the data fetch and shared markup primitives; section bodies live in
//! [`hero`], [`body`], and [`merge`].

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, MergeBooksResult, SuggestionsResponse};

use crate::{data, use_server_url, Route};

mod journal;
mod journal_editor;
mod merge;
mod rating;

// Web renders the split hero + body-grid; mobile re-flows the same loaded-book
// data into a single-column surface. The web-only sections aren't compiled on
// the native shell. (Separate build → no impact on web SSR/WASM parity, rule 07.)
#[cfg(not(feature = "mobile"))]
mod body;
#[cfg(not(feature = "mobile"))]
mod hero;
#[cfg(feature = "mobile")]
mod mobile;

#[cfg(not(feature = "mobile"))]
use body::{BdAuthorCluster, BdBodyMain, BdPageCtx, BdRailSection};
#[cfg(not(feature = "mobile"))]
use hero::BdHeroSection;

/// Book detail page shell: fetches metadata then hands off to `render_loaded`.
#[component]
pub fn BookDetailPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut author_books: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    // F3.3 suggestions. Starts `None` on both SSR and the first WASM paint so
    // hydration markup matches (rule 07); the client effect below populates it.
    let mut suggestions: Signal<Option<SuggestionsResponse>> = use_signal(|| None);
    // Epoch guard so a poll loop left over from a previous book can't write its
    // result onto the current book after navigation (mirrors landing's
    // `fetch_epoch`).
    let mut suggestions_epoch = use_signal(|| 0u64);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    // Bumped after a merge/undo so the effect below refetches the book
    // (signals read inside `use_effect` re-arm it).
    let refresh = use_signal(|| 0u32);

    // Admin gating for the "Merge with…" affordance. The signal and the
    // effect are declared unconditionally so SSR and the hydrating WASM
    // bundle agree on hook count and order — Dioxus tracks hooks
    // positionally, and a cfg-gated declaration would diverge the two.
    // The effect itself only runs on the client (Dioxus skips `use_effect`
    // during SSR), and `data::current_user()` resolves to an
    // `Ok(None)`-returning stub under server-only builds, so no admin
    // surface ever leaks into the prerendered markup. Mobile renders no
    // admin surfaces; the server-side `AdminUser` extractor on
    // `rpc_merge_books` is the actual security boundary.
    let mut is_admin = use_signal(|| false);
    use_effect(move || {
        spawn(async move {
            if let Ok(Some(user)) = data::current_user().await {
                is_admin.set(user.is_admin);
            }
        });
    });

    // Merge dialog state. Declared unconditionally per rule 07 so the hook
    // count is identical on every target — mobile compiles the signals but
    // never reads them (the rsx that consumes them is still cfg-gated below
    // and `build_merge_*` are stubbed to `None` on mobile).
    let merge_open = use_signal(|| false);
    let merge_result: Signal<Option<MergeBooksResult>> = use_signal(|| None);
    let undo_error: Signal<Option<String>> = use_signal(|| None);
    // Mobile's merge builders are `None`-returning stubs that take no args,
    // so the three signals above would otherwise be unused on that target.
    #[cfg(feature = "mobile")]
    let _ = (merge_open, merge_result, undo_error);

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

    // F3.3 suggestions: resolved off-request by the worker and cached. Fetch
    // for the current book; while the result is `Pending`, poll a few times on
    // web so it appears without a manual reload. A fetch failure degrades to
    // "no suggestions" rather than an error row.
    let sug_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let url = sug_url.clone();
        let uuid = uuid.clone();
        let epoch = {
            suggestions_epoch.with_mut(|e| *e += 1);
            *suggestions_epoch.peek()
        };
        // True only while this run is still the latest — a newer book's effect
        // bumps the epoch, so a stale poll drops its result instead of writing
        // it onto the now-current book.
        let is_current = move || *suggestions_epoch.peek() == epoch;
        spawn(async move {
            suggestions.set(None);
            match data::get_suggestions(&url, &uuid).await {
                Ok(resp) => {
                    if !is_current() {
                        return;
                    }
                    let pending = matches!(resp, SuggestionsResponse::Pending);
                    suggestions.set(Some(resp));
                    #[cfg(feature = "web")]
                    if pending {
                        let mut tries = 0u32;
                        while tries < 5 {
                            gloo_timers::future::TimeoutFuture::new(2500).await;
                            if !is_current() {
                                return;
                            }
                            tries += 1;
                            match data::get_suggestions(&url, &uuid).await {
                                Ok(next) => {
                                    if !is_current() {
                                        return;
                                    }
                                    let still_pending =
                                        matches!(next, SuggestionsResponse::Pending);
                                    suggestions.set(Some(next));
                                    if !still_pending {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    #[cfg(not(feature = "web"))]
                    let _ = pending;
                }
                Err(_) => {
                    if is_current() {
                        suggestions.set(Some(SuggestionsResponse::Ready { items: Vec::new() }));
                    }
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

    // `is_admin` starts at `false` and only flips to `true` on the web
    // client after `current_user()` resolves an admin user, so this read
    // returns `false` during SSR and for non-admins on every platform.
    let is_admin_flag = is_admin();

    // Rail "Merge with…" button (admin, web only) — threaded down as a
    // prebuilt Element so the rail component stays platform-agnostic.
    #[cfg(not(feature = "mobile"))]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag, merge_open);
    #[cfg(feature = "mobile")]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let merge_ui: Option<Element> = merge::build_merge_ui(
        merge_open,
        merge_result,
        undo_error,
        refresh,
        server_url.clone(),
        b.clone(),
    );
    #[cfg(feature = "mobile")]
    let merge_ui: Option<Element> = merge::build_merge_ui();

    let body = render_loaded(
        b,
        author_books(),
        merge_button,
        suggestions(),
        server_url.clone(),
        is_admin_flag,
    );
    rsx! {
        {body}
        {merge_ui}
    }
}

// View helpers — the loaded-book case is the only thing rendered, so the
// data-fetch shell above stays small.

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

/// Pre-derived strings + flags ready to feed the loaded-book sections.
/// Split out of [`render_loaded`] so the rsx body stays a thin composition
/// of named sub-components. Mobile reads a subset (no breadcrumb / author-id
/// cluster), so some fields are unused there.
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct LoadedBookView {
    title: String,
    primary_author: String,
    author_id: Option<i64>,
    authors_line: String,
    kicker: String,
    series: Option<String>,
    accent_style: String,
    has_audio: bool,
    has_ebook: bool,
    crumbs: Vec<BdCrumbItem>,
}

/// Compute the per-section display fields from the loaded book.
fn derive_loaded_view(b: &EbookMetadata) -> LoadedBookView {
    let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    let primary_author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let author_id = b.creators.first().and_then(|c| c.id);
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
    let crumbs = build_crumbs(b, &title, &primary_author, &series);
    LoadedBookView {
        title,
        primary_author,
        author_id,
        authors_line,
        kicker,
        series,
        accent_style,
        has_audio,
        has_ebook,
        crumbs,
    }
}

/// Render the fully-loaded book detail view.
fn render_loaded(
    b: EbookMetadata,
    author_books: Vec<EbookMetadata>,
    merge_button: Option<Element>,
    suggestions: Option<SuggestionsResponse>,
    server_url: String,
    is_admin: bool,
) -> Element {
    // Mobile re-flows the same loaded data into a single column; the web body
    // (hero + rail + suggestions + merge UI) isn't rendered there.
    #[cfg(feature = "mobile")]
    let out = {
        let _ = (author_books, merge_button, suggestions, is_admin);
        mobile::render_loaded_mobile(b, server_url)
    };

    #[cfg(not(feature = "mobile"))]
    let out = {
        let LoadedBookView {
            title,
            primary_author,
            author_id,
            authors_line,
            kicker,
            series,
            accent_style,
            has_audio,
            has_ebook,
            crumbs,
        } = derive_loaded_view(&b);

        let uuid = b.unique_identifier.clone().unwrap_or_default();
        rsx! {
            div { class: "bd-root", style: "{accent_style}",
                BdHeroSection {
                    b: b.clone(),
                    title: title.clone(),
                    kicker,
                    crumbs,
                    avail: hero::Availability {
                        has_ebook,
                        has_audio,
                    },
                }
                section { class: "bd-body-grid",
                    BdBodyMain {
                        uuid: uuid.clone(),
                        title: title.clone(),
                        author: BdAuthorCluster { primary_author, author_id, author_books },
                        suggestions,
                        ctx: BdPageCtx { server_url, is_admin },
                    }
                    BdRailSection {
                        b,
                        title,
                        authors_line,
                        series,
                        merge_button,
                    }
                }
                div { class: "bd-footer",
                    Link { to: Route::Landing {}, class: "btn", "Back to library" }
                }
            }
        }
    };

    out
}

// Page-local primitives. None of these introduce business logic — they're
// markup-only adapters so the page reads as a composition of named blocks
// rather than nested rsx.

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
pub(super) fn BdCrumb(items: Vec<BdCrumbItem>) -> Element {
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

/// Body section heading row — kicker label + serif title, with an optional
/// right-aligned `action` slot (mirrors `screens/_shared.jsx#SectionHead`). The
/// kicker stacks above the title; the action floats opposite via the flex
/// `space-between` on `.bd-section-head`.
#[component]
pub(super) fn BdSectionHead(
    kicker: String,
    title: String,
    #[props(default)] action: Option<Element>,
) -> Element {
    rsx! {
        div { class: "bd-section-head",
            div { class: "bd-section-head-text",
                if !kicker.is_empty() {
                    div { class: "label bd-section-kicker", "{kicker}" }
                }
                h3 { class: "bd-section-title", "{title}" }
            }
            {action}
        }
    }
}

/// Small monospace format pill (matches `screens/_shared.jsx#FormatBadge`).
#[component]
pub(super) fn BdFormatBadge(fmt: String) -> Element {
    rsx! {
        div { class: "bd-fmt-badge", "data-testid": "bd-format-badge", "{fmt}" }
    }
}

#[component]
pub(super) fn BdMetaRow(k: String, v: String) -> Element {
    rsx! {
        tr { class: "bd-meta-row",
            td { class: "bd-meta-k", "{k}" }
            td { class: "bd-meta-v", "{v}" }
        }
    }
}

#[component]
pub(super) fn BdInsightCell(label: String, value: String) -> Element {
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
