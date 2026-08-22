//! Immersive CBZ comic pager (`/comic/:uuid`). Pure `<img>` rendering against
//! `/api/ebooks/{uuid}/pages/{n}` — no epub.js, no JS reader library; the
//! browser's image decoder does the work and this module owns the state:
//! current page, n±1 prefetch, fit modes, reading-session capture, and progress
//! mapped onto the Epub-format record via `omnibus_shared::comic_page_anchor`.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{
    comic_page_anchor, parse_comic_page_anchor, EbookMetadata, ProgressFormat, ProgressUpdate,
};

use crate::{data, media_url, use_server_url, Route};

/// How the page image maps onto the stage viewport.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FitMode {
    /// Page fills the stage width; taller pages scroll vertically.
    Width,
    /// Whole page visible, letterboxed to the stage height.
    Height,
}

impl FitMode {
    fn stage_class(self) -> &'static str {
        match self {
            FitMode::Width => "cr-stage cr-fit-width",
            FitMode::Height => "cr-stage cr-fit-height",
        }
    }
}

/// Whole-book percent for a 0-based `page` of `count` — the cross-surface
/// half of the saved position (the landing hero's bar). Last page reads
/// as 100.
fn comic_percent(page: usize, count: usize) -> i64 {
    if count == 0 {
        return 0;
    }
    (((page + 1) * 100) as f64 / count as f64).round() as i64
}

/// The 0-based page a saved record resumes at: the exact
/// [`comic_page_anchor`] when present, else the inverse of
/// [`comic_percent`], else page 0. Clamped so a shrunken re-scan of the
/// archive can't land out of range.
fn start_page(anchor: Option<&str>, percent: Option<i64>, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let last = count - 1;
    if let Some(page) = anchor.and_then(parse_comic_page_anchor) {
        return page.min(last);
    }
    if let Some(pct) = percent {
        let approx = (pct.clamp(0, 100) as f64 / 100.0 * count as f64).round() as usize;
        return approx.saturating_sub(1).min(last);
    }
    0
}

/// Apply a bootstrap fetch's outcome to `meta`/`page`/`error`, dropping it
/// if a later navigation has already bumped `load_seq` past `my_load`.
fn apply_bootstrap_outcome(
    outcome: Option<(EbookMetadata, usize)>,
    my_load: u64,
    load_seq: Signal<u64>,
    mut meta: Signal<Option<EbookMetadata>>,
    mut error: Signal<bool>,
    mut page: Signal<usize>,
) {
    if *load_seq.peek() != my_load {
        return;
    }
    match outcome {
        Some((book, start)) => {
            page.set(start);
            meta.set(Some(book));
        }
        None => error.set(true),
    }
}

/// Resolve a comic's metadata and saved position for the bootstrap effect:
/// `get_ebook`, then (when found) `get_progress` to resolve the starting
/// page via [`start_page`]. `None` on a missing book or a fetch failure —
/// the caller ([`apply_bootstrap_outcome`]) turns that into the error state.
async fn fetch_bootstrap(server_url: &str, uuid: &str) -> Option<(EbookMetadata, usize)> {
    let book = match data::get_ebook(server_url, uuid).await {
        Ok(Some(book)) => book,
        Ok(None) | Err(_) => return None,
    };
    let count = book.page_count.unwrap_or(0).max(0) as usize;
    let saved = data::get_progress(server_url, uuid, ProgressFormat::Epub)
        .await
        .ok()
        .flatten();
    let start = saved
        .map(|r| start_page(r.epub_cfi.as_deref(), r.progress_percent, count))
        .unwrap_or(0);
    Some((book, start))
}

/// Build the progress-save payload for landing on `page` of `count` pages —
/// shared by `goto`'s persist call so the field list can't drift from
/// [`comic_percent`]/`comic_page_anchor`.
fn progress_update_for_page(uuid: &str, page: usize, count: usize) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: Some(comic_page_anchor(page)),
        audio_position_seconds: None,
        progress_percent: Some(comic_percent(page, count)),
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
    }
}

/// Pre-derived, ready-to-render chrome values for one paint — kept in one
/// struct (the `RowDisplay` pattern in `pages/landing/table/row.rs`) so
/// `ComicReadPage`'s body reads declaratively instead of deriving several
/// locals inline.
struct PagerDisplay {
    title: String,
    author: String,
    count: usize,
    pct: i64,
}

impl PagerDisplay {
    fn from_state(meta: Option<&EbookMetadata>, current: usize) -> Self {
        let count = meta.and_then(|m| m.page_count).unwrap_or(0).max(0) as usize;
        Self {
            title: meta.map(|m| m.display_title()).unwrap_or_default(),
            author: meta
                .and_then(|m| m.creators.first().map(|c| c.name.clone()))
                .unwrap_or_default(),
            count,
            pct: if count == 0 {
                0
            } else {
                comic_percent(current, count)
            },
        }
    }
}

/// The full-page comic pager: top bar (back, title, fit modes), the image
/// stage with edge page-turn buttons, and a footer slider + page label.
/// Over the line cap by design: three hooks (session tracking, auto
/// read-status, the bootstrap effect) must run unconditionally before a
/// large declarative rsx! tree mirroring the pager's layout — splitting
/// further would only thread the same signals through another component
/// boundary, the same shape as `BdCtaRow` in `pages/book_detail/hero.rs`.
#[component]
pub fn ComicReadPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();
    let mut meta: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut error: Signal<bool> = use_signal(|| false);
    let mut page: Signal<usize> = use_signal(|| 0);
    let fit: Signal<FitMode> = use_signal(|| FitMode::Height);
    // Monotonic fetch guard (`load_seq` in `read_status_auto::use_auto_read_status`
    // is the model): the router reuses this mounted instance across a
    // same-route uuid change, so a slow response for the previous comic
    // must not land as the current one's state.
    let mut load_seq: Signal<u64> = use_signal(|| 0u64);

    // Record reading time against this comic while the pager is open (and,
    // on web, the tab visible) — the rows behind the `/stats` aggregates,
    // exactly as the EPUB reader does. Effect-only (no rsx), so SSR is a
    // no-op and hydration is unaffected.
    crate::session_tracker::use_reading_session(uuid.clone(), server_url.clone());

    // Auto read-status, the same lifecycle the EPUB reader drives: opening
    // an `Unread` comic marks it `Reading`, and landing on the last page
    // marks it `Finished` (never a downgrade). Effect-only, no rsx.
    let at_end = use_memo(move || {
        let count = meta
            .read()
            .as_ref()
            .and_then(|m| m.page_count)
            .unwrap_or(0)
            .max(0) as usize;
        count > 0 && page() + 1 == count
    });
    crate::read_status_auto::use_auto_read_status(uuid.clone(), server_url.clone(), at_end);
    // Metadata + saved-position bootstrap, post-mount only (rule 07: SSR and
    // the first WASM paint both render the loading state). The restore lands
    // in `page` before `meta` is set so the first painted page is the saved
    // one, never a flash of page 1.
    //
    // `use_reactive!` re-runs this whenever `uuid` changes on an
    // already-mounted instance — the router reuses `ComicReadPage` across a
    // same-route param swap rather than remounting it (#1612) — and
    // `load_seq` (applied via `apply_bootstrap_outcome`) discards a
    // still-in-flight fetch for whichever comic was open before the swap.
    {
        let server_url = server_url.clone();
        use_effect(use_reactive!(|uuid| {
            let my_load = *load_seq.peek() + 1;
            load_seq.set(my_load);
            meta.set(None);
            error.set(false);
            let server_url = server_url.clone();
            spawn(async move {
                let outcome = fetch_bootstrap(&server_url, &uuid).await;
                apply_bootstrap_outcome(outcome, my_load, load_seq, meta, error, page);
            });
        }));
    }

    // Shared page-turn entry point: clamp, render, persist. Every control
    // (buttons, slider, arrow keys) funnels through here so a position is
    // never shown without being saved.
    let goto = {
        let uuid = uuid.clone();
        let server_url = server_url.clone();
        use_callback(move |target: usize| {
            let count = meta
                .peek()
                .as_ref()
                .and_then(|m| m.page_count)
                .unwrap_or(0)
                .max(0) as usize;
            if count == 0 {
                return;
            }
            let clamped = target.min(count - 1);
            if clamped == *page.peek() {
                return;
            }
            page.set(clamped);
            let update = progress_update_for_page(&uuid, clamped, count);
            let server_url = server_url.clone();
            spawn(async move {
                // Best-effort, like the EPUB reader's relocate save: a failed
                // write must not interrupt reading, and the next turn retries.
                let _ = data::save_progress(&server_url, update).await;
            });
        })
    };

    let back_uuid = uuid.clone();
    let on_back = move |_| {
        if nav.can_go_back() {
            nav.go_back();
        } else {
            let _ = nav.push(Route::BookDetail {
                uuid: back_uuid.clone(),
            });
        }
    };

    let current = *page.read();
    let (loaded, display) = {
        let snapshot = meta.read();
        (
            snapshot.is_some(),
            PagerDisplay::from_state(snapshot.as_ref(), current),
        )
    };
    let PagerDisplay {
        title,
        author,
        count,
        pct,
    } = display;
    let page_src = {
        let server_url = server_url.clone();
        let uuid = uuid.clone();
        move |n: usize| media_url(&server_url, &format!("/api/ebooks/{uuid}/pages/{n}"))
    };

    rsx! {
        div {
            class: "cr-root",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |evt: KeyboardEvent| match evt.key() {
                Key::ArrowRight => goto.call(current + 1),
                Key::ArrowLeft if current > 0 => goto.call(current - 1),
                _ => {}
            },
            header { class: "cr-top",
                button {
                    class: "cr-btn cr-back",
                    "data-testid": "comic-back",
                    aria_label: "Back",
                    onclick: on_back,
                    "‹"
                }
                div { class: "cr-title-block",
                    h1 { class: "cr-title", "{title}" }
                    if !author.is_empty() {
                        span { class: "cr-author", "{author}" }
                    }
                }
                div { class: "cr-fit-controls", role: "group", aria_label: "Fit mode",
                    FitButton { fit, mode: FitMode::Width, label: "Fit width", testid: "comic-fit-width" }
                    FitButton { fit, mode: FitMode::Height, label: "Fit height", testid: "comic-fit-height" }
                }
            }
            if *error.read() || (loaded && count == 0) {
                div { class: "cr-state", "data-testid": "comic-error", role: "alert",
                    "This comic could not be opened."
                }
            } else if !loaded {
                div { class: "cr-state", "data-testid": "comic-loading", "Loading comic…" }
            } else {
                div { class: fit.read().stage_class(), "data-testid": "comic-stage",
                    button {
                        class: "cr-nav cr-nav-prev",
                        "data-testid": "comic-prev",
                        aria_label: "Previous page",
                        disabled: current == 0,
                        onclick: move |_| {
                            if current > 0 {
                                goto.call(current - 1);
                            }
                        },
                        "‹"
                    }
                    img {
                        class: "cr-page",
                        key: "{current}",
                        "data-testid": "comic-page-image",
                        src: page_src(current),
                        alt: "Page {current + 1}",
                    }
                    button {
                        class: "cr-nav cr-nav-next",
                        "data-testid": "comic-next",
                        aria_label: "Next page",
                        disabled: current + 1 >= count,
                        onclick: move |_| goto.call(current + 1),
                        "›"
                    }
                    // n±1 prefetch: hidden siblings warm the browser cache so
                    // a turn paints instantly. `ETag` revalidation makes the
                    // later visible fetch a 304, not a second download.
                    if current + 1 < count {
                        img { class: "cr-prefetch", aria_hidden: "true", src: page_src(current + 1) }
                    }
                    if current > 0 {
                        img { class: "cr-prefetch", aria_hidden: "true", src: page_src(current - 1) }
                    }
                }
                footer { class: "cr-bottom", "data-testid": "comic-footer",
                    input {
                        class: "cr-slider",
                        "data-testid": "comic-slider",
                        r#type: "range",
                        min: "0",
                        max: "{count.saturating_sub(1)}",
                        value: "{current}",
                        aria_label: "Page slider",
                        oninput: move |evt| {
                            if let Ok(target) = evt.value().parse::<usize>() {
                                goto.call(target);
                            }
                        },
                        // Arrow keys on a focused slider already step the
                        // range natively (firing `oninput`); without this the
                        // same keydown would bubble to the root handler and
                        // double-turn the page.
                        onkeydown: move |evt| evt.stop_propagation(),
                    }
                    span { class: "cr-page-label", "Page {current + 1} of {count}" }
                    div { class: "cr-ribbon", i { style: "width:{pct}%" } }
                }
            }
        }
    }
}

/// One fit-mode toggle button; `aria-pressed` carries the active state so
/// the pair reads as a proper toggle group.
#[component]
fn FitButton(
    fit: Signal<FitMode>,
    mode: FitMode,
    label: &'static str,
    testid: &'static str,
) -> Element {
    let mut fit = fit;
    let active = *fit.read() == mode;
    rsx! {
        button {
            class: if active { "cr-btn cr-fit active" } else { "cr-btn cr-fit" },
            "data-testid": testid,
            aria_pressed: if active { "true" } else { "false" },
            onclick: move |_| fit.set(mode),
            "{label}"
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod render_tests {
    use dioxus_router::{Routable, Router};

    use super::*;
    use crate::test_support::render_in_vdom;

    // The page calls `use_navigator`, which panics without a parent router,
    // so the test mounts it behind a one-route router at `/` — the
    // highlights-card test pattern.
    #[derive(Clone, Debug, PartialEq, Routable)]
    enum PagerRoute {
        #[route("/")]
        PagerHost {},
    }

    #[component]
    fn PagerHost() -> Element {
        rsx! {
            ComicReadPage { uuid: "some-uuid".to_string() }
        }
    }

    #[test]
    fn comic_read_page_renders_loading_state_before_metadata_arrives() {
        // SSR / first paint: effects never run, so the page must render the
        // loading state — the hydration-parity contract of rule 07.
        let html = render_in_vdom(|| {
            rsx! {
                Router::<PagerRoute> {}
            }
        });
        assert!(html.contains("data-testid=\"comic-loading\""), "{html}");
        assert!(html.contains("data-testid=\"comic-back\""), "{html}");
        assert!(html.contains("data-testid=\"comic-fit-width\""), "{html}");
        assert!(!html.contains("data-testid=\"comic-page-image\""), "{html}");
    }

    // Regression for #1612: the router reuses a mounted `ComicReadPage`
    // across a `/comic/:uuid` param swap instead of remounting it, so two
    // navigations can leave two fetches in flight at once. This drives
    // `apply_bootstrap_outcome` — the exact guard the bootstrap effect calls
    // when each fetch resolves — through that interleaving directly, without
    // needing a live network fetch: navigate to book A, then (before A's
    // fetch would have resolved) navigate on to book B, then land A's
    // now-stale response followed by B's current one.
    #[test]
    fn apply_bootstrap_outcome_drops_a_response_superseded_by_a_newer_navigation() {
        #[component]
        fn AssertBootstrapGuard() -> Element {
            let mut load_seq: Signal<u64> = Signal::new(0);
            let meta: Signal<Option<EbookMetadata>> = Signal::new(None);
            let error: Signal<bool> = Signal::new(false);
            let page: Signal<usize> = Signal::new(0);

            let book_a = EbookMetadata {
                title: Some("Book A".to_string()),
                ..Default::default()
            };
            let book_b = EbookMetadata {
                title: Some("Book B".to_string()),
                ..Default::default()
            };

            // Navigate to A (load 1), then on to B (load 2) before A's fetch
            // has resolved — the router-reuse precondition.
            load_seq.set(1);
            load_seq.set(2);

            // A's fetch lands late: superseded by B's navigation, so it must
            // not populate `meta`.
            apply_bootstrap_outcome(Some((book_a, 5)), 1, load_seq, meta, error, page);
            assert!(
                meta.read().is_none(),
                "a response superseded by a newer navigation must not land"
            );

            // B's fetch lands and is still the current navigation: it must
            // be what renders.
            apply_bootstrap_outcome(Some((book_b, 9)), 2, load_seq, meta, error, page);
            assert_eq!(
                meta.read().as_ref().and_then(|m| m.title.clone()),
                Some("Book B".to_string()),
                "the current navigation's book must render, not the superseded one's"
            );
            assert_eq!(*page.read(), 9);

            rsx! {}
        }
        VirtualDom::new(AssertBootstrapGuard).rebuild_in_place();
    }

    #[test]
    fn apply_bootstrap_outcome_ignores_a_failure_superseded_by_a_newer_navigation() {
        #[component]
        fn AssertBootstrapGuardOnFailure() -> Element {
            let mut load_seq: Signal<u64> = Signal::new(0);
            let meta: Signal<Option<EbookMetadata>> = Signal::new(None);
            let error: Signal<bool> = Signal::new(false);
            let page: Signal<usize> = Signal::new(0);

            load_seq.set(1);
            load_seq.set(2);

            // Load 1's fetch fails after load 2 has already started — must
            // not flip `error` for the book that's no longer open.
            apply_bootstrap_outcome(None, 1, load_seq, meta, error, page);
            assert!(!*error.read(), "a superseded failure must not surface");

            // Load 2's own failure is current and must surface.
            apply_bootstrap_outcome(None, 2, load_seq, meta, error, page);
            assert!(
                *error.read(),
                "the current navigation's failure must surface"
            );

            rsx! {}
        }
        VirtualDom::new(AssertBootstrapGuardOnFailure).rebuild_in_place();
    }

    #[test]
    fn comic_percent_maps_first_and_last_pages_to_ends() {
        assert_eq!(comic_percent(0, 219), 0);
        assert_eq!(comic_percent(218, 219), 100);
        assert_eq!(comic_percent(0, 1), 100);
        assert_eq!(comic_percent(0, 0), 0);
    }

    #[test]
    fn start_page_prefers_the_exact_anchor_and_clamps_it() {
        assert_eq!(start_page(Some("comic-page:41"), Some(10), 219), 41);
        assert_eq!(start_page(Some("comic-page:999"), None, 219), 218);
    }

    #[test]
    fn start_page_falls_back_to_percent_then_zero() {
        // Percent fallback inverts comic_percent within rounding.
        let count = 219;
        let page = 107;
        let pct = comic_percent(page, count);
        let restored = start_page(None, Some(pct), count);
        assert!(
            (restored as i64 - page as i64).abs() <= 1,
            "restored {restored} too far from {page}"
        );
        assert_eq!(start_page(Some("epubcfi(/6/4!/4/2:0)"), None, 219), 0);
        assert_eq!(start_page(None, None, 219), 0);
        assert_eq!(start_page(None, Some(50), 0), 0);
    }
}
