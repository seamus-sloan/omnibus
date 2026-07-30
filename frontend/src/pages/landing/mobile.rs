//! Mobile library layout for the landing page — a compact "All Books" surface
//! (slim header, continue-listening card, Shelves entry, three-column cover
//! grid, sort & filter sheet) rendered by the native shell instead of the web
//! rail + toolbar. Shares [`super::LandingPage`]'s data pipeline; this module
//! owns only the mobile presentation.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, ProgressFormat, ResumePoint, ViewPrefs};

use crate::components::atrium::Cover;
use crate::Route;

use super::mobile_filter_sheet::{dir_arrow, sort_pill_label, MobileSortFilterSheet};
use super::resume_meta::resume_meta;

/// Wait (bounded) for the above-the-fold cover images to decode, then ping
/// Rust so the settled reveal doesn't flash placeholder tiles that covers
/// pop over a few frames later. The 12-image cap keeps the below-the-fold
/// grid out of the gate; the two timeouts bound the whole wait (imgs that
/// never appear — an empty library — and covers that are slow to decode)
/// so this can only delay the reveal, never hold it past the floor.
const COVERS_DECODE_JS: &str = r#"
(async function(){
  function imgs(){
    return Array.prototype.slice.call(document.querySelectorAll('.m-lib img'), 0, 12);
  }
  var appear = Date.now() + 800;
  while (imgs().length === 0 && Date.now() < appear) {
    await new Promise(function(r){ requestAnimationFrame(r); });
  }
  await Promise.race([
    Promise.all(imgs().map(function(i){
      return i.decode ? i.decode().catch(function(){}) : Promise.resolve();
    })),
    new Promise(function(r){ setTimeout(r, 700); })
  ]);
  try { dioxus.send(1); } catch (_e) {}
})();
"#;

/// Load-more paging state for the mobile cover grid: whether the first page
/// is still loading, whether more pages remain, and the fetch-more handler.
/// Grouped so [`MobileLandingProps`] stays under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct MobileLandingPaging {
    /// True while the first page is still loading.
    pub is_loading: bool,
    /// True when more pages remain to load.
    pub has_more: bool,
    /// True while a "Load more" fetch is in flight.
    pub is_loading_more: bool,
    /// Fired when the "Load more" button is pressed.
    pub on_load_more: EventHandler<()>,
}

/// Props for [`MobileLanding`] — the already-derived view state handed
/// down from [`super::LandingPage`].
#[derive(Props, Clone, PartialEq)]
pub(super) struct MobileLandingProps {
    /// Total book count shown in the "N books" label.
    pub book_count: usize,
    /// The page of books to render as cover cells.
    pub books: Vec<EbookMetadata>,
    /// Load-more paging state for the cover grid.
    pub paging: MobileLandingPaging,
    /// Current sort/filter prefs, driving the pill + sheet.
    pub prefs: ViewPrefs,
    /// Fired with updated prefs on every sheet interaction.
    pub on_prefs_change: EventHandler<ViewPrefs>,
    /// Base server URL used to build thumbnail `src`/`srcset`.
    pub server_url: String,
}

/// "Pick up where you left off" — the most recent progress row, fetched per
/// mount and re-read when a background cache revalidation lands.
fn use_resume_point(server_url: String) -> Signal<Option<ResumePoint>> {
    let mut resume = use_signal(|| None::<ResumePoint>);
    let generation = crate::use_cache_generation();
    use_effect(move || {
        let _ = generation();
        let url = server_url.clone();
        spawn(async move {
            if let Ok(points) = crate::data::recent_progress(&url, 1).await {
                resume.set(points.into_iter().next());
            }
        });
    });
    resume
}

/// Settled reveal: hold the screen invisible until the first page fetch lands
/// (the offline cache answers within tens of ms) AND the above-the-fold cover
/// images have decoded, so the enter transition plays once over finished
/// content — no cold "Loading" skeleton fading in, no covers popping over
/// placeholder tiles mid-flight. Every wait is bounded: the decode eval times
/// out internally, and the floor reveals unconditionally so a slow first load
/// falls back to the ordinary Loading state updating in place.
fn use_settled_reveal(is_loading: bool, books_empty: bool) -> bool {
    let mut reveal_floor = use_signal(|| false);
    use_future(move || async move {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        reveal_floor.set(true);
    });
    let mut covers_ready = use_signal(|| false);
    let mut decode_eval = use_hook(|| dioxus::document::eval(COVERS_DECODE_JS));
    use_future(move || async move {
        if decode_eval.recv::<i32>().await.is_ok() {
            covers_ready.set(true);
        }
    });
    (!is_loading && (covers_ready() || books_empty)) || reveal_floor()
}

/// Slim header: brand word + search entry. Account/settings lives on the
/// bottom-nav "You" tab.
fn render_mobile_header() -> Element {
    rsx! {
        header { class: "m-lib-head",
            div { class: "omn-brand-word m-lib-brand", "Omnibus" }
            div { class: "m-lib-head-actions",
                Link {
                    to: Route::MobileSearch {},
                    class: "m-icon-btn",
                    "aria-label": "Search",
                    "data-testid": "mobile-search-entry",
                    {search_glyph()}
                }
            }
        }
    }
}

/// "All Books" title + the sort/filter pill that opens [`MobileSortFilterSheet`].
fn render_mobile_title_row(
    book_count: usize,
    pill_label: &str,
    pill_arrow: &str,
    filter_count: usize,
    sheet_open: Signal<bool>,
) -> Element {
    let mut sheet_open = sheet_open;
    rsx! {
        div { class: "m-lib-title",
            div { class: "m-lib-title-text",
                span { class: "label", "{book_count} books" }
                h2 { class: "m-head-title",
                    "All "
                    span { class: "m-em", "Books" }
                }
            }
            button {
                r#type: "button",
                class: "m-sort-pill",
                "aria-label": "Sort & filter",
                "data-testid": "mobile-sort-pill",
                onclick: move |_| sheet_open.set(true),
                {filter_glyph()}
                span { class: "m-sort-pill-label", "{pill_label}" }
                span { class: "m-sort-pill-arrow", "{pill_arrow}" }
                if filter_count > 0 {
                    span { class: "m-sort-pill-badge mono", "{filter_count}" }
                }
            }
        }
    }
}

/// The cover grid (or the loading line before the first page lands) plus the
/// "Load more" pager.
fn render_mobile_grid(
    is_loading: bool,
    is_loading_more: bool,
    has_more: bool,
    books: Vec<EbookMetadata>,
    server_url: String,
    on_load_more: EventHandler<()>,
) -> Element {
    if is_loading && books.is_empty() {
        return rsx! {
            p { class: "subtitle m-lib-loading", "Loading\u{2026}" }
        };
    }
    rsx! {
        div { class: "m-cover-grid", "data-testid": "mobile-lib-grid", role: "list",
            for book in books.into_iter() {
                {cover_cell(book, &server_url)}
            }
        }
        if has_more {
            button {
                r#type: "button",
                class: "btn m-load-more",
                "data-testid": "mobile-load-more",
                disabled: is_loading_more,
                onclick: move |_| on_load_more.call(()),
                if is_loading_more { "Loading\u{2026}" } else { "Load more" }
            }
        }
    }
}

/// Mobile landing surface. Fed the already-derived view state from
/// [`super::LandingPage`] so the data path stays shared across targets.
#[component]
pub(super) fn MobileLanding(props: MobileLandingProps) -> Element {
    let MobileLandingProps {
        book_count,
        books,
        paging,
        prefs,
        on_prefs_change,
        server_url,
    } = props;
    let MobileLandingPaging {
        is_loading,
        has_more,
        is_loading_more,
        on_load_more,
    } = paging;

    let mut sheet_open = use_signal(|| false);
    let resume = use_resume_point(server_url.clone());

    let pill_label = sort_pill_label(prefs.sort_key);
    let pill_arrow = dir_arrow(prefs.sort_dir);
    let filter_count = prefs.filters.formats.len();

    let books_empty = books.is_empty();
    let settled = use_settled_reveal(is_loading, books_empty);
    let root_class = if settled {
        "m-lib m-lib-ready"
    } else {
        "m-lib"
    };

    rsx! {
        div { class: "{root_class}", "data-testid": "mobile-landing",
            // Pull-to-refresh indicator — dragged/spun imperatively by the
            // JS tracker in `super::pull_refresh`; markup-only here.
            div { class: "m-ptr", "aria-hidden": "true",
                span { class: "m-ptr-arrow", "↓" }
            }
            {render_mobile_header()}
            {render_mobile_title_row(book_count, pill_label, pill_arrow, filter_count, sheet_open)}

            if let Some(point) = resume() {
                {resume_card(&point, &server_url)}
            }

            Link {
                to: Route::Shelves {},
                class: "m-shelves-entry",
                "data-testid": "mobile-shelves-entry",
                span { class: "m-shelves-entry-icon", {bookmark_glyph()} }
                span { class: "m-shelves-entry-body",
                    span { class: "m-shelves-entry-name", "Shelves" }
                    span { class: "m-shelves-entry-sub", "Smart & hand-picked collections" }
                }
                span { class: "m-shelves-entry-chevron", {chevron()} }
            }

            {render_mobile_grid(is_loading, is_loading_more, has_more, books, server_url.clone(), on_load_more)}

            if sheet_open() {
                MobileSortFilterSheet {
                    prefs: prefs.clone(),
                    book_count,
                    on_change: on_prefs_change,
                    on_close: move |_| sheet_open.set(false),
                }
            }
        }
    }
}

/// The "Pick up where you left off" card: cover, title/author, progress
/// meta line and bar, and a play affordance. Tapping resumes in the right
/// surface for the row's format (player for audio, reader for epub).
fn resume_card(point: &ResumePoint, server_url: &str) -> Element {
    let uuid = point.record.book_uuid.clone();
    let book = point.book.clone();
    let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let is_audio = point.record.format == ProgressFormat::Audio;
    let to = crate::routes::resume_route(point);
    let (meta, pct) = resume_meta(point);
    let (src, _srcset) = thumb_srcs(&book, &uuid, server_url);

    rsx! {
        div {
            class: "m-resume",
            "data-testid": "mobile-resume-card",
            span { class: "label m-resume-kicker", "Pick up where you left off" }
            span { class: "m-resume-card",
                Link {
                    to: Route::BookDetail { uuid: uuid.clone() },
                    class: "m-resume-info",
                    span { class: "m-resume-cover",
                        Cover { book, src_override: src, sizes: Some("54px".to_string()) }
                    }
                    span { class: "m-resume-body",
                        span { class: "m-resume-title m-em", "{title}" }
                        if !author.is_empty() {
                            span { class: "m-resume-author", "{author}" }
                        }
                        span { class: "mono m-resume-meta", "{meta}" }
                        if let Some(pct) = pct {
                            span { class: "m-resume-bar", i { style: "width:{pct}%" } }
                        }
                    }
                }
                Link {
                    to,
                    class: "m-resume-play",
                    "data-testid": "mobile-resume-play",
                    "aria-label": "Pick up where you left off: {title}",
                    if is_audio {
                        {play_glyph()}
                    } else {
                        {book_glyph()}
                    }
                }
            }
        }
    }
}

/// One cover cell: cover art + title + author, linking to the detail page.
/// A plain fn (rendered per book) — no hooks, so it can't perturb the parent's
/// hook order. Shared with the mobile shelf-detail grid.
pub(crate) fn cover_cell(book: EbookMetadata, server_url: &str) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);

    rsx! {
        Link {
            key: "{uuid}",
            to: Route::BookDetail { uuid: uuid.clone() },
            class: "m-cover-cell",
            role: "listitem",
            "data-testid": "mobile-lib-tile",
            "aria-label": "Open details for {title}",
            div { class: "m-cover-cell-art",
                Cover {
                    book,
                    src_override: src,
                    srcset,
                    sizes: Some("33vw".to_string()),
                }
                DownloadedBadge { uuid: uuid.clone() }
            }
            div { class: "m-cover-cell-title", "{title}" }
            if !author.is_empty() {
                div { class: "m-cover-cell-author", "{author}" }
            }
        }
    }
}

/// Small check badge over covers of books with a completed offline
/// download. A component (not a plain fn) so each cell subscribes to the
/// download registry and updates live without re-fetching the grid.
#[component]
pub(crate) fn DownloadedBadge(uuid: String) -> Element {
    let initial = crate::offline::downloads::downloaded_uuids().contains(&uuid);
    let mut downloaded = use_signal(move || initial);
    let watch_uuid = uuid.clone();
    use_future(move || {
        let uuid = watch_uuid.clone();
        async move {
            let mut rx = crate::offline::downloads::subscribe();
            loop {
                let now = crate::offline::downloads::downloaded_uuids().contains(&uuid);
                if now != downloaded() {
                    downloaded.set(now);
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    });
    if !downloaded() {
        return rsx! {};
    }
    rsx! {
        span {
            class: "m-cover-dl-badge",
            "aria-label": "Available offline",
            "data-testid": "offline-badge",
            svg {
                width: "10", height: "10", view_box: "0 0 24 24", fill: "none",
                stroke: "currentColor", stroke_width: "3.4", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M20 6L9 17l-5-5" }
            }
        }
    }
}

/// Responsive thumbnail `src`/`srcset` for a book (mirrors the web grid).
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

fn search_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}

fn filter_glyph() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 14 14", fill: "none",
            stroke: "currentColor", stroke_width: "1.5", stroke_linecap: "round",
            path { d: "M2 3.5h10M3.5 7h7M5.5 10.5h3" }
        }
    }
}

fn play_glyph() -> Element {
    rsx! {
        svg {
            width: "15", height: "15", view_box: "0 0 15 15", fill: "currentColor",
            path { d: "M4 2.5v10l8-5z" }
        }
    }
}

fn book_glyph() -> Element {
    rsx! {
        svg {
            width: "15", height: "15", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
        }
    }
}

fn bookmark_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" }
        }
    }
}

fn chevron() -> Element {
    rsx! {
        svg {
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M9 18l6-6-6-6" }
        }
    }
}
