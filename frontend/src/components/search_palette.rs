//! F1.5 Search Palette — command-palette / Spotlight-style search overlay.
//!
//! Replaces the inline `NavSearch` text input with a trigger **button** in the
//! top nav. Clicking the button (or pressing `⌘K` / `Ctrl+K`) opens a floating
//! palette with its own full-width input, debounced FTS5 search, and grouped
//! results: Books, Authors, Series, Tags, and a placeholder "Inside text"
//! section.
//!
//! ## Component tree
//!
//! ```text
//! SearchPaletteHost          — mounted in TopNav, replaces NavSearch
//! ├─ SpTriggerButton         — search button (icon + "Search" + ⌘K kbd hint)
//! └─ (open) SpOverlay        — portal: scrim + panel
//!            ├─ SpInput       — autofocused serif italic input
//!            ├─ SpMeta        — "5 results · 18ms"
//!            ├─ SpResults     — scrollable grouped results
//!            └─ SpFooter      — keyboard hints + "fts5 · ranked"
//! ```
//!
//! ## Hydration safety
//!
//! The palette starts closed (`PaletteOpen = false`). The overlay only renders
//! when open, so SSR and WASM agree on initial DOM — no hydration mismatch.
//! The `⌘K` listener fires only in `#[cfg(feature = "web")]`.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{
    PaletteAuthorHit, PaletteBookHit, PaletteResults, PaletteSeriesHit, PaletteTagHit,
};

use crate::{data, use_search_query, use_server_url, Route};

/// Whether the search palette overlay is open. Registered at the App level
/// via `use_context_provider` so both the trigger button and the global
/// `⌘K` shortcut can toggle it.
#[derive(Copy, Clone, PartialEq)]
pub struct PaletteOpen(pub Signal<bool>);

/// Top-level host: renders the trigger button and (when open) the overlay.
/// Mount this in `TopNav` in place of the old `NavSearch`.
#[component]
pub fn SearchPaletteHost() -> Element {
    let open = use_context::<PaletteOpen>();

    // Register global ⌘K / Ctrl+K listener here (always-mounted) so the
    // shortcut can open the palette from the closed state.
    #[cfg(feature = "web")]
    use_global_shortcut(open);

    rsx! {
        SpTriggerButton { open }
        if open.0() {
            SpOverlay { open }
        }
    }
}

// ── Trigger button ───────────────────────────────────────────────

/// Pill-shaped search button in the nav: search icon + "Search" + ⌘K hint.
#[component]
fn SpTriggerButton(open: PaletteOpen) -> Element {
    let mut open = open;
    rsx! {
        button {
            class: "sp-trigger",
            "data-testid": "search-trigger",
            r#type: "button",
            onclick: move |_| open.0.set(true),
            // Search icon (SVG magnifying glass)
            svg {
                class: "sp-trigger-icon",
                width: "15",
                height: "15",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                circle { cx: "11", cy: "11", r: "8" }
                line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
            }
            span { "Search" }
            kbd { class: "sp-trigger-kbd", "⌘K" }
        }
    }
}

// ── Overlay (scrim + panel) ──────────────────────────────────────

/// Floating overlay: dark scrim + centered panel with input, results, footer.
#[component]
fn SpOverlay(open: PaletteOpen) -> Element {
    let mut open = open;
    let server_url = use_server_url();
    let mut query = use_signal(String::new);
    let mut results = use_signal(|| Option::<PaletteResults>::None);
    let mut selected = use_signal(|| 0_usize);
    let mut loading = use_signal(|| false);
    // Generation counter for debounce — stale responses are discarded.
    let mut generation = use_signal(|| 0_u64);
    let nav = use_navigator();
    let mut search_query = use_search_query();

    // Build a flat list of selectable items for keyboard navigation.
    let flat_items = use_memo(move || build_flat_items(&results.read()));

    // Close the palette.
    let mut close = move || {
        open.0.set(false);
    };

    // Navigate to the selected result.
    let navigate_to_item = {
        move |item: &FlatItem| {
            match item {
                FlatItem::Book { id, .. } => {
                    nav.push(Route::BookDetail { id: *id });
                }
                FlatItem::Author { name, .. } => {
                    search_query.0.set(facet_query("author", name));
                    nav.push(Route::Landing {});
                }
                FlatItem::Series { name, .. } => {
                    search_query.0.set(facet_query("series", name));
                    nav.push(Route::Landing {});
                }
                FlatItem::Tag { name, .. } => {
                    search_query.0.set(facet_query("tag", name));
                    nav.push(Route::Landing {});
                }
            }
            open.0.set(false);
        }
    };

    // Handle keyboard events on the panel.
    let items_for_key = flat_items;
    let mut nav_for_key = navigate_to_item;
    let on_keydown = move |evt: Event<KeyboardData>| {
        let key = evt.key();
        match key {
            Key::Escape => {
                evt.prevent_default();
                open.0.set(false);
            }
            Key::ArrowDown => {
                evt.prevent_default();
                let len = items_for_key.read().len();
                if len > 0 {
                    selected.set((selected() + 1) % len);
                }
            }
            Key::ArrowUp => {
                evt.prevent_default();
                let len = items_for_key.read().len();
                if len > 0 {
                    selected.set(if selected() == 0 {
                        len - 1
                    } else {
                        selected() - 1
                    });
                }
            }
            Key::Enter => {
                evt.prevent_default();
                let items = items_for_key.read();
                if let Some(item) = items.get(selected()) {
                    nav_for_key(item);
                }
            }
            _ => {}
        }
    };

    // Debounced search effect. Uses gloo_timers on web, tokio::time on
    // server. The generation counter ensures stale responses are discarded.
    let url = server_url.clone();
    use_effect(move || {
        let q = query();
        let gen = generation();
        let url = url.clone();
        spawn(async move {
            // 150ms debounce.
            async_sleep_ms(150).await;
            // Stale? Skip.
            if gen != generation() {
                return;
            }
            let trimmed = q.trim().to_string();
            if trimmed.is_empty() {
                results.set(None);
                loading.set(false);
                return;
            }
            loading.set(true);
            match data::search_palette(&url, &trimmed).await {
                Ok(r) => {
                    // Only apply if still current generation.
                    if gen == generation() {
                        selected.set(0);
                        results.set(Some(r));
                    }
                }
                Err(_) => {
                    if gen == generation() {
                        results.set(None);
                    }
                }
            }
            if gen == generation() {
                loading.set(false);
            }
        });
    });

    let res = results.read();
    let items = flat_items.read();
    let sel = selected();
    let is_loading = loading();

    let total = res.as_ref().map(|r| r.total_count()).unwrap_or(0);
    let duration = res.as_ref().map(|r| r.duration_ms).unwrap_or(0);

    rsx! {
        div {
            class: "sp-scrim",
            "data-testid": "sp-scrim",
            onclick: move |_| close(),
            tabindex: "-1",
            div {
                class: "sp-panel",
                "data-testid": "sp-panel",
                role: "dialog",
                aria_label: "Search palette",
                aria_modal: "true",
                // Stop clicks inside the panel from closing the scrim.
                onclick: move |evt| evt.stop_propagation(),
                onkeydown: on_keydown,

                // Input row
                div { class: "sp-input-wrap",
                    svg {
                        class: "sp-input-icon",
                        width: "18",
                        height: "18",
                        view_box: "0 0 24 24",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        circle { cx: "11", cy: "11", r: "8" }
                        line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
                    }
                    input {
                        class: "sp-input",
                        "data-testid": "sp-input",
                        r#type: "text",
                        placeholder: "Search books, authors, series, tags…",
                        autofocus: true,
                        // `autofocus` only fires on initial page load.
                        // `onmounted` programmatically focuses the input
                        // when the overlay is dynamically rendered (⌘K).
                        // Uses `requestAnimationFrame` so the browser has
                        // finished layout before we `.focus()`.
                        onmounted: move |_evt: MountedEvent| {
                            document::eval(r#"
                                requestAnimationFrame(() => {
                                    const el = document.querySelector('[data-testid="sp-input"]');
                                    if (el) el.focus();
                                });
                            "#);
                        },
                        value: "{query}",
                        oninput: move |evt| {
                            let v = evt.value();
                            query.set(v);
                            generation += 1;
                        },
                    }
                    if is_loading {
                        span { class: "sp-spinner", "…" }
                    }
                }

                // Meta line
                if res.is_some() {
                    div { class: "sp-meta", "data-testid": "sp-result-count",
                        "{total} result{plural(total)} · {duration}ms"
                    }
                }

                // Results
                div { class: "sp-results",
                    if let Some(ref r) = *res {
                        // Books
                        if !r.books.is_empty() {
                            SpGroupHead { label: "Books", count: r.books.len() }
                            for book in r.books.iter() {
                                SpBookRow {
                                    book: book.clone(),
                                    selected: is_selected(&items, sel, &FlatItem::Book { id: book.id, title: book.title.clone() }),
                                    on_click: {
                                        let id = book.id;
                                        move |_| {
                                            nav.push(Route::BookDetail { id });
                                            open.0.set(false);
                                        }
                                    },
                                }
                            }
                        }

                        // Authors
                        if !r.authors.is_empty() {
                            SpGroupHead { label: "Authors", count: r.authors.len() }
                            for author in r.authors.iter() {
                                SpAuthorRow {
                                    author: author.clone(),
                                    selected: is_selected(&items, sel, &FlatItem::Author { id: author.id, name: author.name.clone() }),
                                    on_click: {
                                        let name = author.name.clone();
                                        let mut sq = search_query;
                                        move |_| {
                                            sq.0.set(format!("author:{name}"));
                                            nav.push(Route::Landing {});
                                            open.0.set(false);
                                        }
                                    },
                                }
                            }
                        }

                        // Series
                        if !r.series.is_empty() {
                            SpGroupHead { label: "Series", count: r.series.len() }
                            for s in r.series.iter() {
                                SpSeriesRow {
                                    series: s.clone(),
                                    selected: is_selected(&items, sel, &FlatItem::Series { id: s.id, name: s.name.clone() }),
                                    on_click: {
                                        let name = s.name.clone();
                                        let mut sq = search_query;
                                        move |_| {
                                            sq.0.set(format!("series:{name}"));
                                            nav.push(Route::Landing {});
                                            open.0.set(false);
                                        }
                                    },
                                }
                            }
                        }

                        // Tags
                        if !r.tags.is_empty() {
                            SpGroupHead { label: "Tags", count: r.tags.len() }
                            for tag in r.tags.iter() {
                                SpTagRow {
                                    tag: tag.clone(),
                                    selected: is_selected(&items, sel, &FlatItem::Tag { id: tag.id, name: tag.name.clone() }),
                                    on_click: {
                                        let name = tag.name.clone();
                                        let mut sq = search_query;
                                        move |_| {
                                            sq.0.set(format!("tag:{name}"));
                                            nav.push(Route::Landing {});
                                            open.0.set(false);
                                        }
                                    },
                                }
                            }
                        }

                        // Inside text — placeholder
                        SpGroupHead { label: "Inside text", count: 0 }
                        div { class: "sp-coming-soon", "data-testid": "sp-coming-soon",
                            "Coming soon"
                        }
                    }
                }

                // Footer
                SpFooter {}
            }
        }
    }
}

// ── Result rows ──────────────────────────────────────────────────

#[component]
fn SpGroupHead(label: &'static str, count: usize) -> Element {
    rsx! {
        div { class: "sp-group-head label",
            if count > 0 {
                "{label} · {count}"
            } else {
                "{label}"
            }
        }
    }
}

#[component]
fn SpBookRow(book: PaletteBookHit, selected: bool, on_click: EventHandler<MouseEvent>) -> Element {
    let sel_class = if selected {
        "sp-row selected"
    } else {
        "sp-row"
    };
    let server_url = use_server_url();
    let cover = if book.cover_url.is_some() {
        let url = format!("{server_url}/api/thumbs/{}/sm", book.id);
        rsx! {
            img {
                class: "sp-row-cover",
                src: "{url}",
                alt: "",
                loading: "lazy",
            }
        }
    } else {
        // Accent-backed fallback with first letter
        let initial = book
            .title
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        let bg = book.accent.as_deref().unwrap_or("var(--bg-2)");
        rsx! {
            div {
                class: "sp-row-cover sp-row-cover-fallback",
                style: "background: {bg};",
                "{initial}"
            }
        }
    };

    let year = book.year.as_deref().unwrap_or("");
    let formats: String = book.formats.join(" · ");

    rsx! {
        div {
            class: "{sel_class}",
            "data-testid": "sp-book-row",
            onclick: move |evt| on_click.call(evt),
            {cover}
            div { class: "sp-row-body",
                div { class: "sp-row-title", "{book.title}" }
                div { class: "sp-row-sub", "{book.author_display}" }
            }
            if !year.is_empty() || !formats.is_empty() {
                div { class: "sp-row-meta",
                    if !year.is_empty() { span { "{year}" } }
                    if !formats.is_empty() { span { "{formats}" } }
                }
            }
        }
    }
}

#[component]
fn SpAuthorRow(
    author: PaletteAuthorHit,
    selected: bool,
    on_click: EventHandler<MouseEvent>,
) -> Element {
    let sel_class = if selected {
        "sp-row selected"
    } else {
        "sp-row"
    };
    let initial = author
        .name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    rsx! {
        div {
            class: "{sel_class}",
            "data-testid": "sp-author-row",
            onclick: move |evt| on_click.call(evt),
            div { class: "sp-avatar", "{initial}" }
            div { class: "sp-row-body",
                div { class: "sp-row-title", "{author.name}" }
                div { class: "sp-row-sub",
                    "{author.book_count} book{plural(author.book_count as usize)}"
                }
            }
        }
    }
}

#[component]
fn SpSeriesRow(
    series: PaletteSeriesHit,
    selected: bool,
    on_click: EventHandler<MouseEvent>,
) -> Element {
    let sel_class = if selected {
        "sp-row selected"
    } else {
        "sp-row"
    };

    rsx! {
        div {
            class: "{sel_class}",
            "data-testid": "sp-series-row",
            onclick: move |evt| on_click.call(evt),
            div { class: "sp-avatar", "S" }
            div { class: "sp-row-body",
                div { class: "sp-row-title", "{series.name}" }
                div { class: "sp-row-sub",
                    "{series.book_count} book{plural(series.book_count as usize)}"
                    if let Some(ref author) = series.author_display {
                        " · {author}"
                    }
                }
            }
        }
    }
}

#[component]
fn SpTagRow(tag: PaletteTagHit, selected: bool, on_click: EventHandler<MouseEvent>) -> Element {
    let sel_class = if selected {
        "sp-row selected"
    } else {
        "sp-row"
    };

    rsx! {
        div {
            class: "{sel_class}",
            "data-testid": "sp-tag-row",
            onclick: move |evt| on_click.call(evt),
            span { class: "sp-tag-chip", "# {tag.name}" }
            div { class: "sp-row-body",
                div { class: "sp-row-sub",
                    "{tag.book_count} book{plural(tag.book_count as usize)}"
                }
            }
        }
    }
}

#[component]
fn SpFooter() -> Element {
    rsx! {
        div { class: "sp-footer",
            div { class: "sp-footer-keys",
                kbd { "↑↓" }
                span { " navigate" }
                kbd { "⏎" }
                span { " open" }
                kbd { "esc" }
                span { " close" }
            }
            div { class: "sp-footer-engine",
                "fts5 · ranked by relevance"
            }
        }
    }
}

// ── Flat item model for keyboard nav ─────────────────────────────

/// A single selectable item in the flat list used for arrow-key navigation.
#[derive(Clone, Debug, PartialEq)]
enum FlatItem {
    Book { id: i64, title: String },
    Author { id: i64, name: String },
    Series { id: i64, name: String },
    Tag { id: i64, name: String },
}

/// Build a flat ordered list of all selectable items from the results.
fn build_flat_items(results: &Option<PaletteResults>) -> Vec<FlatItem> {
    let Some(r) = results else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for b in &r.books {
        items.push(FlatItem::Book {
            id: b.id,
            title: b.title.clone(),
        });
    }
    for a in &r.authors {
        items.push(FlatItem::Author {
            id: a.id,
            name: a.name.clone(),
        });
    }
    for s in &r.series {
        items.push(FlatItem::Series {
            id: s.id,
            name: s.name.clone(),
        });
    }
    for t in &r.tags {
        items.push(FlatItem::Tag {
            id: t.id,
            name: t.name.clone(),
        });
    }
    items
}

/// Check if a given flat item matches the currently selected index.
fn is_selected(items: &[FlatItem], selected_idx: usize, candidate: &FlatItem) -> bool {
    items.get(selected_idx) == Some(candidate)
}

/// Simple English plural suffix.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Build a facet query string where every whitespace-separated word in
/// `value` is prefixed with `prefix:`. This ensures `build_fts_match`
/// routes each token to the correct FTS5 column filter instead of
/// treating trailing words as free-text (e.g. `tag:Dark tag:academia`
/// rather than `tag:Dark academia`).
fn facet_query(prefix: &str, value: &str) -> String {
    value
        .split_whitespace()
        .map(|w| format!("{prefix}:{w}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Async sleep (platform-gated) ─────────────────────────────────

/// Platform-gated async sleep. Web uses `gloo_timers`, server uses `tokio`.
#[cfg(feature = "web")]
async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(all(not(feature = "web"), feature = "server"))]
async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(all(not(feature = "web"), not(feature = "server")))]
async fn async_sleep_ms(_ms: u32) {}

// ── Global ⌘K shortcut (web only) ───────────────────────────────

/// Register a global `keydown` listener that toggles the palette on `⌘K`
/// (Mac) or `Ctrl+K` (other platforms). Only compiled for the web target
/// so SSR builds don't pull in `web_sys`.
#[cfg(feature = "web")]
fn use_global_shortcut(open: PaletteOpen) {
    let mut open = open;
    // use_hook runs exactly once per component instance (not on re-renders),
    // so the closure is registered once and leaked once — no duplicate listeners.
    use_hook(move || {
        use wasm_bindgen::prelude::*;

        let closure = Closure::wrap(Box::new(move |evt: web_sys::KeyboardEvent| {
            let is_cmd_k = (evt.meta_key() || evt.ctrl_key()) && evt.key() == "k";
            if is_cmd_k {
                evt.prevent_default();
                let current = open.0();
                open.0.set(!current);
            }
        }) as Box<dyn FnMut(_)>);

        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
        }

        // Leak the closure so it lives for the app lifetime. The shortcut
        // is registered once (use_hook guarantees this) and never removed —
        // acceptable for a single-page app.
        closure.forget();
    });
}
