//! Book detail page — Atrium "Cinematic" shell.
//!
//! Owns the data fetch and shared markup primitives; section bodies live in
//! [`hero`], [`body`], and [`merge`].

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, MergeBooksResult, SuggestionsResponse};

use crate::components::{PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

mod delete;
mod discovery;
mod file_picker;
mod immersive;
mod journal;
mod journal_editor;
mod merge;
mod rating;
mod read_status;

// Web renders the split hero + body-grid; mobile re-flows the same loaded-book
// data into a single-column surface. The web-only sections aren't compiled on
// the native shell. (Separate build → no impact on web SSR/WASM parity, rule 07.)
#[cfg(not(feature = "mobile"))]
mod body;
#[cfg(not(feature = "mobile"))]
mod export_menu;
#[cfg(not(feature = "mobile"))]
mod hero;
#[cfg(feature = "mobile")]
mod mobile;
#[cfg(feature = "mobile")]
mod offline;
#[cfg(not(feature = "mobile"))]
mod physical;

#[cfg(not(feature = "mobile"))]
use body::{BdAuthorCluster, BdBodyMain, BdPageCtx, BdRailSection};
#[cfg(not(feature = "mobile"))]
use hero::BdHeroSection;

/// Book detail page shell: fetches metadata then hands off to `render_loaded`.
#[component]
pub fn BookDetailPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    // Reflect the loaded book title in the browser tab (e.g. "Omnibus | Dracula").
    crate::use_page_title(move || book.read().as_ref().and_then(|b| b.title.clone()));
    let author_books: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    // F3.3 suggestions. Starts `None` on both SSR and the first WASM paint so
    // hydration markup matches (rule 07); the client effect below populates it.
    let suggestions: Signal<Option<SuggestionsResponse>> = use_signal(|| None);
    // Epoch guard so a poll loop left over from a previous book can't write its
    // result onto the current book after navigation (mirrors landing's
    // `fetch_epoch`).
    let suggestions_epoch = use_signal(|| 0u64);
    let loading = use_signal(|| true);
    let error: Signal<Option<String>> = use_signal(|| None);
    // Bumped after a merge/undo so the effect below refetches the book
    // (signals read inside `use_effect` re-arm it).
    let refresh = use_signal(|| 0u32);

    // Admin gating for the "Merge with…" affordance — derived from the
    // app-wide `CurrentUser` context (`crate::use_is_admin`) instead of an
    // independent per-mount `/api/auth/me` fetch. Mobile/SSR stay at the
    // `false` default since the context is web-only; the server-side
    // `AdminUser` extractor on `rpc_merge_books` is the actual security
    // boundary.
    let is_admin = crate::use_is_admin();

    // Merge dialog state. Declared unconditionally per rule 07 so the hook
    // count is identical on every target — mobile compiles the signals but
    // `render_book_shell` stubs the builders that read them.
    let merge_open = use_signal(|| false);
    let merge_result: Signal<Option<MergeBooksResult>> = use_signal(|| None);
    let undo_error: Signal<Option<String>> = use_signal(|| None);
    // Delete-dialog state, declared unconditionally for the same reason.
    let delete_open = use_signal(|| false);

    // Fetch Summary override (mobile only). Declared unconditionally per
    // rule 07 — `render_loaded_mobile` is a plain fn with no hook scope,
    // reached only past the early returns below. `epoch` is bumped
    // alongside `value` on every fresh book load; the Fetch Summary save in
    // `render_loaded_mobile` checks it's unchanged before writing, so a save
    // left over from a since-navigated-away book can't clobber the new
    // book's description.
    let description = DescriptionSignals {
        value: use_signal(String::new),
        epoch: use_signal(|| 0u64),
    };

    use_book_data_effects(
        uuid.clone(),
        server_url.clone(),
        BookDataSignals {
            book,
            author_books,
            loading,
            error,
            refresh,
            suggestions_epoch,
            suggestions,
            description,
        },
    );

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }
    let Some(b) = book() else {
        return rsx! { PageNotFound { subject: "Book", back_to: Route::Landing {} } };
    };

    render_book_shell(
        b,
        MergeSignals {
            open: merge_open,
            result: merge_result,
            undo_error,
            refresh,
        },
        PageSignals {
            description,
            delete_open,
        },
        author_books(),
        suggestions(),
        is_admin,
        server_url,
    )
}

/// Fetch Summary override signals (mobile only): the effective description
/// plus the epoch that guards a pending fetch-and-save against resolving
/// onto a different, later-navigated book. `Copy` (Dioxus signals) and
/// grouped so call sites stay under the function-argument cap.
#[derive(Clone, Copy)]
struct DescriptionSignals {
    value: Signal<String>,
    epoch: Signal<u64>,
}

/// Extra per-page state threaded from [`BookDetailPage`]'s prologue into
/// [`render_book_shell`]: the Fetch Summary override (mobile) and the
/// delete-dialog open toggle (web). Grouped only to keep that call site
/// under clippy's argument cap. `Copy` (Dioxus signals), so passing by
/// value is cheap.
#[derive(Clone, Copy)]
struct PageSignals {
    description: DescriptionSignals,
    delete_open: Signal<bool>,
}

/// The book-page data signals written by the fetch effects. `Copy` (Dioxus
/// signals), so the group is passed by value.
#[derive(Clone, Copy)]
struct BookDataSignals {
    book: Signal<Option<EbookMetadata>>,
    author_books: Signal<Vec<EbookMetadata>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    refresh: Signal<u32>,
    suggestions_epoch: Signal<u64>,
    suggestions: Signal<Option<SuggestionsResponse>>,
    description: DescriptionSignals,
}

/// Install the two `uuid`-reactive fetch effects: the book + author-books load
/// (also re-armed by `refresh` after a merge/undo) and the suggestions
/// poll. Called unconditionally from [`BookDetailPage`] so the hook sequence
/// stays fixed; both effects re-run when `uuid` changes.
fn use_book_data_effects(uuid: String, server_url: String, sig: BookDataSignals) {
    let url = server_url.clone();
    let refresh = sig.refresh;
    let generation = crate::use_cache_generation();
    use_effect(use_reactive!(|uuid| {
        // Read `refresh` so a merge/undo bump re-arms this effect, and the
        // cache generation so a background revalidation refreshes the page
        // (served from the fresh cache, zero network).
        let _ = refresh();
        let _ = generation();
        fetch_book_and_author_books(
            url.clone(),
            uuid.clone(),
            sig.book,
            sig.author_books,
            sig.loading,
            sig.error,
            sig.description,
        );
    }));

    // F3.3 suggestions: resolved off-request by the worker and cached. Fetch
    // for the current book; while the result is `Pending`, poll a few times on
    // web so it appears without a manual reload. A fetch failure degrades to
    // "no suggestions" rather than an error row.
    let sug_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        poll_suggestions_until_resolved(
            sug_url.clone(),
            uuid.clone(),
            sig.suggestions_epoch,
            sig.suggestions,
        );
    }));
}

/// Merge-dialog signals threaded from [`BookDetailPage`] into the loaded-book
/// renderer. `Copy` (Dioxus signals), so passing them by value is cheap and
/// keeps the callee free of extra hook calls.
#[derive(Clone, Copy)]
struct MergeSignals {
    open: Signal<bool>,
    result: Signal<Option<MergeBooksResult>>,
    undo_error: Signal<Option<String>>,
    refresh: Signal<u32>,
}

/// Prebuilt web-only rail action buttons (Merge / Delete), threaded from
/// [`render_book_shell`] into [`render_loaded`]. Grouped only to keep that
/// call site under clippy's argument cap; both are always `None` on mobile,
/// and mobile discards the whole struct rather than reading its fields (see
/// [`LoadedBookView`] for the same pattern).
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct RailButtons {
    merge: Option<Element>,
    delete: Option<Element>,
}

/// Assemble the loaded-book view: derive the admin flag, build the (web-only)
/// merge button + dialog, and compose them with [`render_loaded`]. Kept a plain
/// fn so the platform `cfg` gates live here instead of the component body
/// (rule 07 — no new gates inside `BookDetailPage`).
fn render_book_shell(
    b: EbookMetadata,
    merge: MergeSignals,
    page: PageSignals,
    author_books: Vec<EbookMetadata>,
    suggestions: Option<SuggestionsResponse>,
    is_admin: ReadSignal<bool>,
    server_url: String,
) -> Element {
    let PageSignals {
        description,
        delete_open,
    } = page;

    // `is_admin` starts at `false` and only flips to `true` on the web
    // client after `current_user()` resolves an admin user, so this read
    // returns `false` during SSR and for non-admins on every platform.
    let is_admin_flag = is_admin();

    // Rail "Merge with…" button (admin, web only) — threaded down as a
    // prebuilt Element so the rail component stays platform-agnostic.
    #[cfg(not(feature = "mobile"))]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag, merge.open);
    #[cfg(feature = "mobile")]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let merge_ui: Option<Element> = merge::build_merge_ui(
        merge.open,
        merge.result,
        merge.undo_error,
        merge.refresh,
        server_url.clone(),
        b.clone(),
    );
    #[cfg(feature = "mobile")]
    let merge_ui: Option<Element> = {
        // Mobile's builders take no args; read every field so the signals
        // (declared unconditionally in `BookDetailPage` for hook parity)
        // aren't flagged as never-read on this target.
        let MergeSignals {
            open,
            result,
            undo_error,
            refresh,
        } = merge;
        let _ = (open, result, undo_error, refresh);
        merge::build_merge_ui()
    };

    // Rail "Delete files…" button + its dialog, same web-only shape as merge.
    #[cfg(not(feature = "mobile"))]
    let delete_button: Option<Element> = delete::build_delete_button(is_admin_flag, delete_open);
    #[cfg(feature = "mobile")]
    let delete_button: Option<Element> = delete::build_delete_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let delete_ui: Option<Element> = delete::build_delete_ui(
        delete_open,
        merge.refresh,
        b.unique_identifier.clone().unwrap_or_default(),
        b.display_title(),
    );
    #[cfg(feature = "mobile")]
    let delete_ui: Option<Element> = {
        let _ = delete_open;
        delete::build_delete_ui()
    };

    let body = render_loaded(
        b,
        description,
        author_books,
        RailButtons {
            merge: merge_button,
            delete: delete_button,
        },
        suggestions,
        LoadedCtx {
            server_url,
            is_admin: is_admin_flag,
            refresh: merge.refresh,
        },
    );
    rsx! {
        {body}
        {merge_ui}
        {delete_ui}
    }
}

// View helpers — the loaded-book case is the only thing rendered, so the
// data-fetch shell above stays small.

/// Fetch the book, then (if it resolves and has a creator) the primary
/// author's other books, filtering out the current book. `book` and
/// `author_books` are written directly so a fast re-run (uuid change) is
/// naturally superseded by the newer fetch's writes. `description.value`
/// (mobile's Fetch Summary override) resets to the freshly-loaded book's
/// saved summary on every call, and `description.epoch` bumps alongside it
/// so a fetch-and-save left over from a previous book can detect it's stale
/// and drop its result instead of overwriting the new book's description.
fn fetch_book_and_author_books(
    server_url: String,
    uuid: String,
    mut book: Signal<Option<EbookMetadata>>,
    mut author_books: Signal<Vec<EbookMetadata>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
    mut description: DescriptionSignals,
) {
    spawn(async move {
        // Keep the rendered page in place when re-fetching the *same* book
        // (a cache-revalidation bump) — the skeleton and author-rail reset
        // are for actual book changes. `unique_identifier` is the book uuid
        // on API rows.
        let same_book = book
            .peek()
            .as_ref()
            .and_then(|b| b.unique_identifier.as_deref())
            == Some(uuid.as_str());
        if !same_book {
            loading.set(true);
            author_books.set(Vec::new());
        }
        match data::get_ebook(&server_url, &uuid).await {
            Ok(b) => {
                let author_fetch = b.as_ref().map(|inner| {
                    (
                        inner.creators.first().and_then(|c| c.id),
                        inner.unique_identifier.clone(),
                    )
                });
                description.value.set(
                    b.as_ref()
                        .and_then(|inner| inner.description.clone())
                        .unwrap_or_default(),
                );
                description.epoch.with_mut(|e| *e += 1);
                book.set(b);
                error.set(None);
                loading.set(false);
                if let Some((Some(aid), current_uuid)) = author_fetch {
                    if let Ok(Some(ad)) = data::get_author(&server_url, aid).await {
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
}

/// Fetch suggestions for `uuid` and, on web, poll a few times while the
/// result is `Pending`. `suggestions_epoch` is bumped once per call and
/// captured as the run's identity, so a stale poll left over from a previous
/// book (fast SPA navigation) drops its result instead of overwriting the
/// now-current book's suggestions.
fn poll_suggestions_until_resolved(
    server_url: String,
    uuid: String,
    mut suggestions_epoch: Signal<u64>,
    mut suggestions: Signal<Option<SuggestionsResponse>>,
) {
    let epoch = {
        suggestions_epoch.with_mut(|e| *e += 1);
        *suggestions_epoch.peek()
    };
    let is_current = move || *suggestions_epoch.peek() == epoch;
    spawn(async move {
        suggestions.set(None);
        match data::get_suggestions(&server_url, &uuid).await {
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
                        match data::get_suggestions(&server_url, &uuid).await {
                            Ok(next) => {
                                if !is_current() {
                                    return;
                                }
                                let still_pending = matches!(next, SuggestionsResponse::Pending);
                                suggestions.set(Some(next));
                                if !still_pending {
                                    break;
                                }
                            }
                            Err(_) => {
                                // Mirror the initial-fetch error path below: fall
                                // back to the empty-ready state so the UI doesn't
                                // get stuck on "loading suggestions" forever.
                                if is_current() {
                                    suggestions.set(Some(SuggestionsResponse::Ready {
                                        items: Vec::new(),
                                    }));
                                }
                                break;
                            }
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
}

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
    let title = b.display_title();
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

/// Server URL + admin flag + book-refetch signal threaded into
/// [`render_loaded`]. Grouped so the call site stays under clippy's argument
/// cap; `Copy`-free (`server_url` is a `String`), so it's passed by value once.
struct LoadedCtx {
    server_url: String,
    is_admin: bool,
    refresh: Signal<u32>,
}

/// Render the fully-loaded book detail view.
fn render_loaded(
    b: EbookMetadata,
    description: DescriptionSignals,
    author_books: Vec<EbookMetadata>,
    rail: RailButtons,
    suggestions: Option<SuggestionsResponse>,
    ctx: LoadedCtx,
) -> Element {
    let LoadedCtx {
        server_url,
        is_admin,
        refresh,
    } = ctx;
    // Mobile re-flows the same loaded data into a single column; the web body
    // (hero + rail + suggestions + merge UI) isn't rendered there.
    #[cfg(feature = "mobile")]
    let out = {
        // The merge and delete affordances stay web-only; the discovery blocks
        // (author cluster + suggestions) now render on mobile too. The physical
        // panel is web-only too (issue #1186 scope), so `refresh` is unused here.
        let _ = rail;
        let _ = refresh;
        mobile::render_loaded_mobile(mobile::MobileBookView {
            b,
            author_books,
            suggestions,
            is_admin,
            server_url,
            description,
        })
    };

    #[cfg(not(feature = "mobile"))]
    let out = {
        // Web keeps its own local copy inside `BdTitleCol` (hero.rs), a real
        // `#[component]` that gets a fresh scope per mount — no hook-order
        // risk there, so this param is unused on this target.
        let _ = description;
        let RailButtons {
            merge: merge_button,
            delete: delete_button,
        } = rail;
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
        // Extract the physical-panel inputs before `b` is moved into the rail.
        let is_fileless = b.formats.is_empty();
        let isbn13 = b.isbn13.clone();
        let panel_author = primary_author.clone();
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
                physical::BdPhysicalPanel {
                    uuid: uuid.clone(),
                    is_fileless,
                    isbn: isbn13,
                    title: title.clone(),
                    author: panel_author,
                    refresh,
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
                        delete_button,
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
                Fragment { key: "{i}-{item.text}",
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
