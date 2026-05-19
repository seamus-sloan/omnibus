use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{
    Contributor, EbookLibrary, EbookMetadata, SortDir, SortKey, ViewFilters, ViewMode, ViewPrefs,
};

use crate::{data, use_search_query, use_server_url, view_prefs, Route};

/// Landing page — primary library surface.
///
/// Hydrates the configured ebook library once, then renders either a dense
/// table or a cover grid. Sort and filter happen entirely client-side over
/// the hydrated list (per F1.3 spec for libraries up to ~10k books). View
/// mode + sort + filters persist per library path via [`view_prefs`].
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    let mut library = use_signal(EbookLibrary::default);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut prefs = use_signal(ViewPrefs::default);
    // Search box lives in the top nav; the query is shared via context.
    let query = use_search_query().0;

    // Fetch the library when the search query changes.
    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let url = url_for_fetch.clone();
        let q = query();
        spawn(async move {
            loading.set(true);
            let trimmed = q.trim();
            let result = if trimmed.is_empty() {
                data::get_ebooks(&url).await
            } else {
                data::search_ebooks(&url, trimmed).await
            };
            match result {
                Ok(lib) => {
                    library.set(lib);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    // Hydrate persisted prefs whenever the library path resolves.
    use_effect(move || {
        if let Some(path) = library.read().path.clone() {
            let stored = view_prefs::load(&path);
            if stored != prefs.peek().clone() {
                prefs.set(stored);
            }
        }
    });

    // Memoize the two O(N) derivations so unrelated re-renders (the
    // `loading` flag flipping, search-query churn that doesn't change the
    // hydrated list) don't re-walk every book. `use_memo` re-runs only
    // when a signal it reads changes — so `facets` is keyed implicitly on
    // `library`, and `visible` on `library + prefs` (filters + sort).
    let facets = use_memo(move || facet_counts(&library.read().books));
    let visible = use_memo(move || {
        let p = prefs();
        sort_books(
            apply_filters(&library.read().books, &p.filters),
            p.sort_key,
            p.sort_dir,
        )
    });

    let lib = library();
    let is_loading = loading();
    let page_error = error();
    let book_count = lib.books.len();
    let view_mode = prefs().view_mode;
    let visible_books = visible();
    let visible_is_empty = visible_books.is_empty();
    let facet_counts_view = facets();

    let server_url_for_row = server_url.clone();
    let path_for_save = lib.path.clone();
    let save = {
        let path = path_for_save.clone();
        move |new_prefs: ViewPrefs| {
            if let Some(path) = path.as_ref() {
                view_prefs::save(path, &new_prefs);
            }
            prefs.set(new_prefs);
        }
    };

    let path_subtitle = lib.path.as_ref().map(|p| short_path(p)).unwrap_or_default();
    let visible_count = visible_books.len();
    let filters_for_chips = prefs().filters.clone();
    let total_formats: usize = facet_counts_view.formats.iter().map(|(_, c)| c).sum();

    rsx! {
        header { class: "lib-header", "data-testid": "lib-header",
            div { class: "lib-header-kicker",
                // Semantic page title — kept visually small (label-style) so
                // the cinematic count below reads as the dominant element,
                // but assistive tech and `getByRole("heading", { level: 1 })`
                // still find a stable "Your Library" anchor.
                h1 { class: "label lib-header-kicker-title", "Your Library" }
                if !path_subtitle.is_empty() {
                    span { class: "mono lib-header-path", " · {path_subtitle}" }
                }
            }
            div { class: "lib-header-row",
                p { class: "lib-header-title",
                    em { "{book_count}" }
                    " "
                    if book_count == 1 { "book" } else { "books" }
                }
                Toolbar {
                    prefs: prefs(),
                    on_change: save.clone(),
                }
            }
            if lib.path.is_none() {
                p { class: "lib-header-hint",
                    "Configure your ebook library path in Settings."
                }
            }
            if let Some(msg) = page_error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
            if let Some(msg) = lib.error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
        }

        FormatChips {
            counts: facet_counts_view.formats.clone(),
            total: total_formats,
            visible_count: visible_count,
            book_count: book_count,
            selected: filters_for_chips.formats.clone(),
            on_change: {
                let mut save = save.clone();
                move |formats: Vec<String>| {
                    let mut next = prefs.peek().clone();
                    next.filters.formats = formats;
                    save(next);
                }
            },
        }

        div { class: if prefs().filters_open { "lib-layout" } else { "lib-layout lib-layout--collapsed" },
            FilterSidebar {
                facets: facet_counts_view,
                filters: prefs().filters.clone(),
                on_change: {
                    let mut save = save.clone();
                    move |filters: ViewFilters| {
                        let mut next = prefs.peek().clone();
                        next.filters = filters;
                        save(next);
                    }
                },
            }

            div { class: "lib-main",
                if is_loading {
                    p { class: "library-empty", "Loading..." }
                } else if !visible_is_empty || lib.error.is_some() || page_error.is_some() {
                    match view_mode {
                        ViewMode::Table => rsx! {
                            BookTable {
                                books: visible_books.clone(),
                                prefs: prefs(),
                                on_sort: {
                                    let mut save = save.clone();
                                    move |key: SortKey| {
                                        let mut next = prefs.peek().clone();
                                        next.sort_dir = if next.sort_key == key {
                                            toggle_dir(next.sort_dir)
                                        } else {
                                            default_dir_for(key)
                                        };
                                        next.sort_key = key;
                                        save(next);
                                    }
                                },
                                server_url: server_url_for_row.clone(),
                            }
                        },
                        ViewMode::Grid => rsx! {
                            BookGrid {
                                books: visible_books.clone(),
                            }
                        },
                    }
                } else if lib.books.is_empty() {
                    p { class: "library-empty", "No ebooks found." }
                } else {
                    EmptyFiltered {
                        on_clear: {
                            let mut save = save.clone();
                            move |_| {
                                let mut next = prefs.peek().clone();
                                next.filters = ViewFilters::default();
                                save(next);
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Short, human-friendly tail of an absolute library path. We show only the
/// last segment to keep the header line tidy — full path lives in Settings.
fn short_path(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

#[component]
fn Toolbar(prefs: ViewPrefs, on_change: EventHandler<ViewPrefs>) -> Element {
    let view_mode = prefs.view_mode;
    let sort_key = prefs.sort_key;
    let sort_dir = prefs.sort_dir;
    let filters_open = prefs.filters_open;

    let apply = move |new_prefs: ViewPrefs| on_change.call(new_prefs);
    let set_view = {
        let prefs = prefs.clone();
        move |mode: ViewMode| {
            let mut next = prefs.clone();
            next.view_mode = mode;
            apply(next);
        }
    };
    let toggle_filters = {
        let prefs = prefs.clone();
        move |_| {
            let mut next = prefs.clone();
            next.filters_open = !next.filters_open;
            apply(next);
        }
    };
    let set_sort_key = {
        let prefs = prefs.clone();
        move |key: SortKey| {
            let mut next = prefs.clone();
            // Switching to a different axis from the grid dropdown should
            // adopt that axis's natural direction (descending for time-based
            // axes, ascending for alphabetical) — matches the table-view
            // header behavior so the two views stay consistent.
            if next.sort_key != key {
                next.sort_dir = default_dir_for(key);
            }
            next.sort_key = key;
            apply(next);
        }
    };
    let toggle_sort_dir = {
        let prefs = prefs.clone();
        move |_| {
            let mut next = prefs.clone();
            next.sort_dir = toggle_dir(next.sort_dir);
            apply(next);
        }
    };

    let set_view_table = set_view.clone();
    let set_view_grid = set_view.clone();

    rsx! {
        div { class: "lib-toolbar", role: "toolbar", "data-testid": "lib-toolbar",
            button {
                class: "lib-toggle-btn lib-filters-btn",
                "aria-pressed": "{filters_open}",
                "data-testid": "lib-filters-toggle",
                aria_label: "Toggle filter sidebar",
                onclick: toggle_filters,
                "Filters"
            }
            // Pressed-button toggle group, not an ARIA tablist — there are no
            // associated tab panels and no arrow-key tab navigation, so
            // `aria-pressed` on plain `<button>`s is the right shape.
            div { class: "lib-view-toggle", "aria-label": "View mode",
                button {
                    class: "lib-toggle-btn",
                    "aria-pressed": "{view_mode == ViewMode::Table}",
                    "data-testid": "view-toggle-table",
                    onclick: move |_| set_view_table(ViewMode::Table),
                    "Table"
                }
                button {
                    class: "lib-toggle-btn",
                    "aria-pressed": "{view_mode == ViewMode::Grid}",
                    "data-testid": "view-toggle-grid",
                    onclick: move |_| set_view_grid(ViewMode::Grid),
                    "Grid"
                }
            }

            if view_mode == ViewMode::Grid {
                div { class: "lib-sort-controls",
                    label { class: "lib-sort-label",
                        "Sort by"
                        select {
                            class: "lib-sort-select",
                            "data-testid": "lib-sort-select",
                            onchange: move |evt: Event<FormData>| {
                                if let Some(key) = sort_key_from_value(&evt.value()) {
                                    set_sort_key(key);
                                }
                            },
                            for opt in SORT_KEYS.iter().copied() {
                                option {
                                    value: "{sort_key_value(opt)}",
                                    selected: opt == sort_key,
                                    "{sort_key_label(opt)}"
                                }
                            }
                        }
                    }
                    button {
                        class: "lib-sort-dir",
                        "data-testid": "lib-sort-dir",
                        aria_label: "Toggle sort direction",
                        onclick: toggle_sort_dir,
                        if sort_dir == SortDir::Asc { "↑" } else { "↓" }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Filter sidebar
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
struct FacetCounts {
    authors: Vec<(String, usize)>,
    series: Vec<(String, usize)>,
    /// Normalized lowercase format keys (`"epub"`, `"m4b"`, …) paired with
    /// counts. The display label is derived at render time via
    /// [`format_display_label`] so the keys stay canonical.
    formats: Vec<(String, usize)>,
}

#[component]
fn FilterSidebar(
    facets: FacetCounts,
    filters: ViewFilters,
    on_change: EventHandler<ViewFilters>,
) -> Element {
    let any_active = !filters.authors.is_empty() || !filters.series.is_empty();
    let toggle = {
        let filters = filters.clone();
        move |group: &'static str, value: String| {
            let mut next = filters.clone();
            let bucket = if group == "authors" {
                &mut next.authors
            } else {
                &mut next.series
            };
            if let Some(pos) = bucket.iter().position(|v| v == &value) {
                bucket.remove(pos);
            } else {
                bucket.push(value);
            }
            on_change.call(next);
        }
    };

    rsx! {
        aside { class: "lib-sidebar", "data-testid": "lib-sidebar", aria_label: "Filters",
            if any_active {
                button {
                    class: "lib-clear-filters",
                    "data-testid": "lib-clear-filters",
                    onclick: move |_| on_change.call(ViewFilters::default()),
                    "Clear filters"
                }
            }

            FacetSection {
                title: "Authors",
                testid: "lib-facet-authors",
                items: facets.authors.clone(),
                selected: filters.authors.clone(),
                on_toggle: {
                    let toggle = toggle.clone();
                    move |v: String| toggle("authors", v)
                },
            }
            FacetSection {
                title: "Series",
                testid: "lib-facet-series",
                items: facets.series.clone(),
                selected: filters.series.clone(),
                on_toggle: {
                    let toggle = toggle.clone();
                    move |v: String| toggle("series", v)
                },
            }
        }
    }
}

#[component]
fn FacetSection(
    title: String,
    testid: String,
    items: Vec<(String, usize)>,
    selected: Vec<String>,
    on_toggle: EventHandler<String>,
) -> Element {
    if items.is_empty() {
        return rsx! { Fragment {} };
    }
    let selected_set: HashSet<&String> = selected.iter().collect();
    rsx! {
        section { class: "lib-facet", "data-testid": "{testid}",
            h3 { class: "lib-facet-title", "{title}" }
            ul { class: "lib-chip-list",
                for (name, count) in items.iter() {
                    li {
                        button {
                            // Layer Atrium's `.chip` look onto the existing
                            // `.lib-chip` class — the Playwright selector
                            // `button.lib-chip[data-value="…"]` still matches.
                            class: "chip lib-chip",
                            "aria-pressed": "{selected_set.contains(&name)}",
                            "data-value": "{name}",
                            title: "{name}",
                            onclick: {
                                let name = name.clone();
                                move |_| on_toggle.call(name.clone())
                            },
                            span { class: "lib-chip-label", "{name}" }
                            span { class: "count lib-chip-count", "{count}" }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Format chips (top-of-page inline filter)
// ---------------------------------------------------------------------------

#[component]
fn FormatChips(
    counts: Vec<(String, usize)>,
    total: usize,
    visible_count: usize,
    book_count: usize,
    selected: Vec<String>,
    on_change: EventHandler<Vec<String>>,
) -> Element {
    if counts.is_empty() {
        return rsx! { Fragment {} };
    }
    let selected_set: HashSet<String> = selected.iter().cloned().collect();
    let all_active = selected.is_empty();

    rsx! {
        div { class: "lib-format-chips",
            "data-testid": "lib-format-chips",
            role: "group",
            aria_label: "Format filters",

            span { class: "label lib-format-chips-label", "Filter" }

            button {
                class: if all_active { "chip on" } else { "chip" },
                "data-format": "all",
                "aria-pressed": "{all_active}",
                onclick: move |_| on_change.call(Vec::new()),
                "All formats "
                span { class: "count", "{total}" }
            }

            for (key, count) in counts.into_iter() {
                {
                    let is_selected = selected_set.contains(&key);
                    let label = format_display_label(&key);
                    let key_for_click = key.clone();
                    let selected_for_click = selected.clone();
                    rsx! {
                        button {
                            class: if is_selected { "chip on" } else { "chip" },
                            "data-format": "{key}",
                            "aria-pressed": "{is_selected}",
                            onclick: move |_| {
                                let mut next: Vec<String> = selected_for_click.clone();
                                if let Some(pos) = next.iter().position(|v| v == &key_for_click) {
                                    next.remove(pos);
                                } else {
                                    next.push(key_for_click.clone());
                                }
                                on_change.call(next);
                            },
                            "{label} "
                            span { class: "count", "{count}" }
                        }
                    }
                }
            }

            div { class: "lib-format-chips-spacer" }
            span { class: "mono lib-format-chips-count",
                "{visible_count} of {book_count}"
            }
        }
    }
}

#[component]
fn EmptyFiltered(on_clear: EventHandler<()>) -> Element {
    rsx! {
        div { class: "library-empty",
            p { "No books match these filters." }
            button {
                class: "btn",
                "data-testid": "lib-clear-filters-empty",
                onclick: move |_| on_clear.call(()),
                "Clear filters"
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Table view
// ---------------------------------------------------------------------------

#[component]
fn BookTable(
    books: Vec<EbookMetadata>,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
    server_url: String,
) -> Element {
    rsx! {
        div {
            id: "ebook-table",
            "data-testid": "ebook-table",
            class: "ebook-table-wrap",
            table { class: "ebook-table",
                thead {
                    tr {
                        th { class: "ebook-col-cover", "Cover" }
                        SortableHeader {
                            class: "ebook-col-title".to_string(),
                            label: "Title".to_string(),
                            sort_key: SortKey::Title,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-author".to_string(),
                            label: "Author".to_string(),
                            sort_key: SortKey::Author,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-series".to_string(),
                            label: "Series".to_string(),
                            sort_key: SortKey::Series,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-publisher", "Publisher" }
                        th { class: "ebook-col-published", "Published" }
                        th { class: "ebook-col-formats", "Formats" }
                        SortableHeader {
                            class: "ebook-col-updated".to_string(),
                            label: "Last Updated".to_string(),
                            sort_key: SortKey::LastUpdated,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-added".to_string(),
                            label: "Added".to_string(),
                            sort_key: SortKey::NewestAdded,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-language", "Language" }
                    }
                }
                tbody {
                    for book in books.into_iter() {
                        EbookRow {
                            key: "{book.filename}",
                            book: book,
                            server_url: server_url.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SortableHeader(
    class: String,
    label: String,
    sort_key: SortKey,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
) -> Element {
    let active = prefs.sort_key == sort_key;
    let aria_sort = match (active, prefs.sort_dir) {
        (true, SortDir::Asc) => "ascending",
        (true, SortDir::Desc) => "descending",
        _ => "none",
    };
    let arrow = if !active {
        ""
    } else if prefs.sort_dir == SortDir::Asc {
        " ↑"
    } else {
        " ↓"
    };
    rsx! {
        th { class: "{class} sort-th", aria_sort: "{aria_sort}",
            button {
                class: "sort-th-btn",
                onclick: move |_| on_sort.call(sort_key),
                "{label}{arrow}"
            }
        }
    }
}

#[component]
fn EbookRow(book: EbookMetadata, server_url: String) -> Element {
    let id = book.id;
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let row_testid = format!("ebook-row-{}", row_slug(&book.filename));
    let has_cover = book.cover_url.is_some();
    let thumb_base = format!("{server_url}/api/thumbs/{}", book.id);
    let series_line = match (book.series.as_deref(), book.series_index.as_deref()) {
        (Some(s), Some(i)) => format!("{s} #{i}"),
        (Some(s), None) => s.to_string(),
        _ => String::new(),
    };
    let authors = contributor_names(&book.creators);
    let updated = book.modified.as_deref().unwrap_or("").to_string();
    let added = book.added_at.as_deref().unwrap_or("").to_string();

    let nav = use_navigator();

    rsx! {
        tr {
            class: "ebook-row",
            "data-testid": "{row_testid}",
            id: "{row_testid}",
            role: "button",
            tabindex: "0",
            aria_label: "Open details for {display_title}",
            onclick: move |_| {
                nav.push(Route::BookDetail { id });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                let key = evt.key();
                if key == Key::Enter || key == Key::Character(" ".to_string()) {
                    evt.prevent_default();
                    nav.push(Route::BookDetail { id });
                }
            },
            td { class: "ebook-col-cover", "data-testid": "ebook-cell-cover",
                if has_cover {
                    img {
                        class: "ebook-thumb",
                        src: "{thumb_base}/md",
                        srcset: "{thumb_base}/sm 160w, {thumb_base}/md 320w, {thumb_base}/lg 640w",
                        sizes: "(max-width: 640px) 160px, (max-width: 1280px) 320px, 640px",
                        alt: "Cover of {display_title}",
                        loading: "lazy",
                        width: "320",
                        height: "480",
                    }
                } else {
                    div { class: "ebook-thumb ebook-thumb-fallback", "—" }
                }
            }
            td { class: "ebook-col-title", "data-testid": "ebook-cell-title",
                div { class: "ebook-title-cell", "{display_title}" }
                if let Some(err) = book.error.as_ref() {
                    div { class: "error", "⚠ {err}" }
                }
            }
            td { class: "ebook-col-author", "data-testid": "ebook-cell-author", "{authors}" }
            td { class: "ebook-col-series", "data-testid": "ebook-cell-series", "{series_line}" }
            td { class: "ebook-col-publisher", "data-testid": "ebook-cell-publisher", {book.publisher.as_deref().unwrap_or("")} }
            td { class: "ebook-col-published", "data-testid": "ebook-cell-published", {book.published.as_deref().unwrap_or("")} }
            td { class: "ebook-col-formats", "data-testid": "ebook-cell-formats",
                if book.formats.is_empty() {
                    span { class: "ebook-cell-formats-empty", "—" }
                } else {
                    for fmt in book.formats.iter() {
                        span { class: "format-badge", "{format_badge_label(fmt)}" }
                    }
                }
            }
            td { class: "ebook-col-updated", "data-testid": "ebook-cell-updated", "{updated}" }
            td { class: "ebook-col-added", "data-testid": "ebook-cell-added", "{added}" }
            td { class: "ebook-col-language", "data-testid": "ebook-cell-language", {book.language.as_deref().unwrap_or("")} }
        }
    }
}

// ---------------------------------------------------------------------------
// Grid view
// ---------------------------------------------------------------------------

#[component]
fn BookGrid(books: Vec<EbookMetadata>) -> Element {
    rsx! {
        div { class: "lib-grid", "data-testid": "lib-grid", role: "list",
            for book in books.into_iter() {
                GridTile {
                    key: "{book.filename}",
                    book: book,
                }
            }
        }
    }
}

#[component]
fn GridTile(book: EbookMetadata) -> Element {
    let id = book.id;
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let tile_testid = format!("ebook-tile-{}", row_slug(&book.filename));
    let authors = contributor_names(&book.creators);
    let book_for_cover = book.clone();
    let nav = use_navigator();

    rsx! {
        a {
            class: "cover-link lib-tile",
            "data-testid": "{tile_testid}",
            role: "listitem",
            tabindex: "0",
            aria_label: "Open details for {display_title}",
            onclick: move |_| { nav.push(Route::BookDetail { id }); },
            onkeydown: move |evt: Event<KeyboardData>| {
                let key = evt.key();
                if key == Key::Enter || key == Key::Character(" ".to_string()) {
                    evt.prevent_default();
                    nav.push(Route::BookDetail { id });
                }
            },
            crate::components::atrium::Cover { book: book_for_cover }
            div { class: "lib-tile-title", "{display_title}" }
            if !authors.is_empty() {
                div { class: "lib-tile-author", "{authors}" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers — exercised by tests below.
// ---------------------------------------------------------------------------

fn contributor_names(list: &[Contributor]) -> String {
    let mut out = String::new();
    for (i, c) in list.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&c.name);
    }
    out
}

fn primary_author_key(book: &EbookMetadata) -> String {
    let c = book.creators.first();
    let name = c
        .map(|c| c.file_as.as_deref().unwrap_or(&c.name).to_string())
        .unwrap_or_default();
    name.to_ascii_lowercase()
}

fn title_key(book: &EbookMetadata) -> String {
    let t = book.title.as_deref().unwrap_or(&book.filename);
    t.to_ascii_lowercase()
}

/// Cached per-row sort key. We compute exactly one of these (matching the
/// active [`SortKey`]) per book before sorting, then `sort_by` only borrows
/// pre-built strings — no per-comparison allocation, no re-parsing of
/// `series_index`. `series_index` is normalized to milli-units of an i64 so
/// the whole struct is `Ord`-derivable (no f64 NaN issues).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RowKey {
    /// Plain string axes (Title / Author / LastUpdated / NewestAdded).
    /// `None` only for genuinely missing values; see [`cmp_with_missing_last`].
    plain: Option<String>,
    /// Series tuple: lowercased name + `series_index * 1000` rounded to i64.
    series: Option<(String, i64)>,
}

fn row_key(book: &EbookMetadata, key: SortKey) -> RowKey {
    match key {
        SortKey::Title => RowKey {
            plain: Some(title_key(book)),
            series: None,
        },
        SortKey::Author => RowKey {
            plain: Some(primary_author_key(book)),
            series: None,
        },
        SortKey::Series => RowKey {
            plain: None,
            series: book.series.as_deref().filter(|s| !s.is_empty()).map(|s| {
                let idx = book
                    .series_index
                    .as_deref()
                    .and_then(|raw| raw.parse::<f64>().ok())
                    .map(|f| (f * 1000.0).round() as i64)
                    .unwrap_or(0);
                (s.to_ascii_lowercase(), idx)
            }),
        },
        SortKey::LastUpdated => RowKey {
            plain: book.modified.clone(),
            series: None,
        },
        SortKey::NewestAdded => RowKey {
            plain: book.added_at.clone(),
            series: None,
        },
    }
}

/// Compare two `Option<K>` values where missing always sorts last regardless
/// of direction. Direction only flips ordering between two present values;
/// `None` keeps a stable "last" position so reversing a desc sort doesn't
/// shove un-timestamped or seriesless books to the top.
fn cmp_with_missing_last<K: Ord>(a: Option<&K>, b: Option<&K>, dir: SortDir) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => {
            let ord = x.cmp(y);
            if dir == SortDir::Desc {
                ord.reverse()
            } else {
                ord
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn sort_books(books: Vec<EbookMetadata>, key: SortKey, dir: SortDir) -> Vec<EbookMetadata> {
    let mut keyed: Vec<(RowKey, EbookMetadata)> =
        books.into_iter().map(|b| (row_key(&b, key), b)).collect();
    keyed.sort_by(|(ka, ba), (kb, bb)| {
        let primary = match key {
            SortKey::Series => cmp_with_missing_last(ka.series.as_ref(), kb.series.as_ref(), dir),
            _ => cmp_with_missing_last(ka.plain.as_ref(), kb.plain.as_ref(), dir),
        };
        // Stable tiebreak on id, never reversed — keeps run-to-run order
        // deterministic when the primary key matches.
        primary.then(ba.id.cmp(&bb.id))
    });
    keyed.into_iter().map(|(_, b)| b).collect()
}

fn matches_filters(book: &EbookMetadata, filters: &ViewFilters) -> bool {
    // Allocation-free membership checks: filter buckets are typically tiny
    // (a handful of selected chips), so a nested `any().any()` is faster
    // than building a fresh HashSet per book on every filter pass.
    if !filters.authors.is_empty()
        && !filters
            .authors
            .iter()
            .any(|a| book.creators.iter().any(|c| &c.name == a))
    {
        return false;
    }
    if !filters.series.is_empty() {
        let series = book.series.as_deref().unwrap_or("");
        if !filters.series.iter().any(|s| s == series) {
            return false;
        }
    }
    if !filters.formats.is_empty()
        && !filters
            .formats
            .iter()
            .any(|f| book.formats.iter().any(|bf| bf.eq_ignore_ascii_case(f)))
    {
        return false;
    }
    true
}

fn apply_filters(books: &[EbookMetadata], filters: &ViewFilters) -> Vec<EbookMetadata> {
    if filters.is_empty() {
        return books.to_vec();
    }
    books
        .iter()
        .filter(|b| matches_filters(b, filters))
        .cloned()
        .collect()
}

fn facet_counts(books: &[EbookMetadata]) -> FacetCounts {
    let mut authors: BTreeMap<String, usize> = BTreeMap::new();
    let mut series: BTreeMap<String, usize> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    for book in books {
        for c in &book.creators {
            *authors.entry(c.name.clone()).or_default() += 1;
        }
        if let Some(s) = book.series.as_deref() {
            if !s.is_empty() {
                *series.entry(s.to_string()).or_default() += 1;
            }
        }
        for fmt in &book.formats {
            let key = fmt.trim().to_ascii_lowercase();
            if !key.is_empty() {
                *formats.entry(key).or_default() += 1;
            }
        }
    }
    FacetCounts {
        authors: sorted_facet(authors),
        series: sorted_facet(series),
        formats: sorted_facet(formats),
    }
}

/// User-facing label for a normalized format key. Recognized formats get a
/// friendly name (`"epub"` → `"ePub"`, `"m4b"` → `"Audiobook"`); anything
/// else passes through upper-cased.
fn format_display_label(key: &str) -> String {
    match key {
        "epub" => "ePub".to_string(),
        "m4b" => "Audiobook".to_string(),
        "pdf" => "PDF".to_string(),
        "mp3" => "Audiobook (MP3)".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Short badge text for the table's Formats column. Stays compact so a row
/// with two formats doesn't overflow the cell.
fn format_badge_label(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

fn sorted_facet(map: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn toggle_dir(d: SortDir) -> SortDir {
    match d {
        SortDir::Asc => SortDir::Desc,
        SortDir::Desc => SortDir::Asc,
    }
}

fn default_dir_for(key: SortKey) -> SortDir {
    // "Newest Added" / "Last Updated" feel natural with newest first.
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

const SORT_KEYS: [SortKey; 5] = [
    SortKey::Title,
    SortKey::Author,
    SortKey::Series,
    SortKey::LastUpdated,
    SortKey::NewestAdded,
];

fn sort_key_value(key: SortKey) -> &'static str {
    match key {
        SortKey::Title => "title",
        SortKey::Author => "author",
        SortKey::Series => "series",
        SortKey::LastUpdated => "last_updated",
        SortKey::NewestAdded => "newest_added",
    }
}

fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Title => "Title",
        SortKey::Author => "Author",
        SortKey::Series => "Series",
        SortKey::LastUpdated => "Last Updated",
        SortKey::NewestAdded => "Newest Added",
    }
}

fn sort_key_from_value(value: &str) -> Option<SortKey> {
    match value {
        "title" => Some(SortKey::Title),
        "author" => Some(SortKey::Author),
        "series" => Some(SortKey::Series),
        "last_updated" => Some(SortKey::LastUpdated),
        "newest_added" => Some(SortKey::NewestAdded),
        _ => None,
    }
}

/// Stable Playwright row id derived from the ebook's on-disk filename:
/// strip directories and extension, lowercase, then collapse runs of
/// non-alphanumeric ASCII characters into a single `-` (with leading and
/// trailing dashes trimmed). The Playwright fixture table mirrors this
/// derivation so each `FIXTURE_BOOKS[i].slug` matches the row's testid.
fn row_slug(filename: &str) -> String {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let stem = basename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(basename);
    let lower = stem.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_dash = true;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::Contributor;

    #[allow(clippy::too_many_arguments)]
    fn book(
        id: i64,
        filename: &str,
        title: Option<&str>,
        authors: &[(&str, Option<&str>)],
        series: Option<(&str, &str)>,
        modified: Option<&str>,
        added_at: Option<&str>,
        subjects: &[&str],
    ) -> EbookMetadata {
        EbookMetadata {
            id,
            filename: filename.into(),
            title: title.map(Into::into),
            creators: authors
                .iter()
                .map(|(name, file_as)| Contributor {
                    name: (*name).into(),
                    role: None,
                    file_as: file_as.map(Into::into),
                })
                .collect(),
            series: series.map(|(s, _)| s.into()),
            series_index: series.map(|(_, i)| i.into()),
            modified: modified.map(Into::into),
            added_at: added_at.map(Into::into),
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    fn ids(books: &[EbookMetadata]) -> Vec<i64> {
        books.iter().map(|b| b.id).collect()
    }

    // --- row_slug ---

    #[test]
    fn row_slug_lowercases_and_strips_extension() {
        assert_eq!(row_slug("Alpha.epub"), "alpha");
    }
    #[test]
    fn row_slug_collapses_runs_of_non_alphanumerics() {
        assert_eq!(row_slug("Beta in the Series.epub"), "beta-in-the-series");
    }
    #[test]
    fn row_slug_uses_basename_for_nested_paths() {
        assert_eq!(row_slug("series/vol1/Deep Book.epub"), "deep-book");
    }
    #[test]
    fn row_slug_trims_trailing_dashes() {
        assert_eq!(row_slug("weird---name!!!.epub"), "weird-name");
    }
    #[test]
    fn row_slug_handles_filename_without_extension() {
        assert_eq!(row_slug("plain"), "plain");
    }

    // --- sort_books ---

    fn sample() -> Vec<EbookMetadata> {
        vec![
            book(
                1,
                "alpha.epub",
                Some("Alpha"),
                &[("Tolkien, J.R.R.", Some("Tolkien, J.R.R."))],
                Some(("Foundation", "1")),
                Some("2024-01-01T00:00:00"),
                Some("2025-03-10T00:00:00"),
                &["Fantasy"],
            ),
            book(
                2,
                "beta.epub",
                Some("Beta"),
                &[("Asimov, Isaac", Some("Asimov, Isaac"))],
                Some(("Foundation", "2")),
                Some("2024-06-01T00:00:00"),
                Some("2025-01-05T00:00:00"),
                &["Sci-Fi"],
            ),
            book(
                3,
                "gamma.epub",
                Some("Gamma"),
                &[("Le Guin, Ursula", Some("Le Guin, Ursula"))],
                None,
                Some("2023-01-01T00:00:00"),
                Some("2025-02-20T00:00:00"),
                &["Fantasy", "Sci-Fi"],
            ),
        ]
    }

    #[test]
    fn sorts_by_title_asc_and_desc() {
        let s = sample();
        let asc = sort_books(s.clone(), SortKey::Title, SortDir::Asc);
        assert_eq!(ids(&asc), vec![1, 2, 3]);
        let desc = sort_books(s, SortKey::Title, SortDir::Desc);
        assert_eq!(ids(&desc), vec![3, 2, 1]);
    }

    #[test]
    fn sorts_by_author_asc() {
        let s = sample();
        let asc = sort_books(s, SortKey::Author, SortDir::Asc);
        // Asimov < Le Guin < Tolkien
        assert_eq!(ids(&asc), vec![2, 3, 1]);
    }

    #[test]
    fn sorts_by_series_grouping_with_index_then_pushes_seriesless_last() {
        let s = sample();
        let asc = sort_books(s, SortKey::Series, SortDir::Asc);
        // Foundation #1 (id 1), Foundation #2 (id 2), then no-series (id 3).
        assert_eq!(ids(&asc), vec![1, 2, 3]);
    }

    #[test]
    fn sorts_by_last_updated_desc_picks_most_recent_first() {
        let s = sample();
        let desc = sort_books(s, SortKey::LastUpdated, SortDir::Desc);
        // beta 2024-06 > alpha 2024-01 > gamma 2023
        assert_eq!(ids(&desc), vec![2, 1, 3]);
    }

    #[test]
    fn sorts_by_newest_added_desc() {
        let s = sample();
        let desc = sort_books(s, SortKey::NewestAdded, SortDir::Desc);
        // alpha 2025-03 > gamma 2025-02 > beta 2025-01
        assert_eq!(ids(&desc), vec![1, 3, 2]);
    }

    #[test]
    fn missing_timestamps_always_sort_last_even_on_desc() {
        // Two timestamped books + one with no `modified` value. In descending
        // order the most-recent timestamp comes first, but the missing-value
        // book stays at the end (it doesn't get flipped to the top by the
        // direction reversal).
        let books = vec![
            book(
                1,
                "old.epub",
                Some("Old"),
                &[],
                None,
                Some("2024-01-01T00:00:00"),
                None,
                &[],
            ),
            book(
                2,
                "new.epub",
                Some("New"),
                &[],
                None,
                Some("2025-01-01T00:00:00"),
                None,
                &[],
            ),
            book(
                3,
                "missing.epub",
                Some("Missing"),
                &[],
                None,
                None,
                None,
                &[],
            ),
        ];
        let desc = sort_books(books.clone(), SortKey::LastUpdated, SortDir::Desc);
        assert_eq!(ids(&desc), vec![2, 1, 3]);
        let asc = sort_books(books, SortKey::LastUpdated, SortDir::Asc);
        assert_eq!(ids(&asc), vec![1, 2, 3]);
    }

    #[test]
    fn series_sort_keeps_seriesless_last_in_desc_too() {
        let s = sample();
        let desc = sort_books(s, SortKey::Series, SortDir::Desc);
        // Foundation #2 (id 2) → Foundation #1 (id 1) → seriesless gamma (id 3)
        // last regardless of direction.
        assert_eq!(ids(&desc), vec![2, 1, 3]);
    }

    // --- apply_filters ---

    #[test]
    fn empty_filters_returns_all_books() {
        let s = sample();
        let out = apply_filters(&s, &ViewFilters::default());
        assert_eq!(ids(&out), vec![1, 2, 3]);
    }

    #[test]
    fn single_facet_or_within_group() {
        let s = sample();
        let f = ViewFilters {
            authors: vec!["Tolkien, J.R.R.".into(), "Asimov, Isaac".into()],
            ..Default::default()
        };
        let out = apply_filters(&s, &f);
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn multi_facet_and_across_groups() {
        let s = sample();
        let f = ViewFilters {
            authors: vec!["Tolkien, J.R.R.".into(), "Asimov, Isaac".into()],
            series: vec!["Foundation".into()],
            ..Default::default()
        };
        let out = apply_filters(&s, &f);
        // alpha is Tolkien + Foundation, beta is Asimov + Foundation, gamma
        // is Le Guin with no series — so only alpha + beta survive the AND
        // across (authors, series).
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn series_filter_excludes_books_with_no_series() {
        let s = sample();
        let f = ViewFilters {
            series: vec!["Foundation".into()],
            ..Default::default()
        };
        let out = apply_filters(&s, &f);
        assert_eq!(ids(&out), vec![1, 2]);
    }

    // --- facet_counts ---

    #[test]
    fn facet_counts_orders_by_count_desc_then_name() {
        let s = sample();
        let f = facet_counts(&s);
        // Series: Foundation present once with count 2
        assert_eq!(f.series, vec![("Foundation".into(), 2)]);
        // Authors: each unique once
        assert_eq!(f.authors.len(), 3);
    }

    #[test]
    fn facet_counts_skips_empty_series_strings() {
        let mut b = sample();
        b[0].series = Some(String::new());
        let f = facet_counts(&b);
        assert!(f.series.iter().all(|(s, _)| !s.is_empty()));
    }

    // --- format filter ---

    fn with_formats(mut b: EbookMetadata, formats: &[&str]) -> EbookMetadata {
        b.formats = formats.iter().map(|s| (*s).to_string()).collect();
        b
    }

    #[test]
    fn format_counts_normalize_case_insensitively() {
        let books = vec![
            with_formats(sample()[0].clone(), &["EPUB"]),
            with_formats(sample()[1].clone(), &["epub", "m4b"]),
            with_formats(sample()[2].clone(), &["M4B"]),
        ];
        let f = facet_counts(&books);
        let formats: std::collections::HashMap<_, _> = f.formats.into_iter().collect();
        assert_eq!(formats.get("epub").copied(), Some(2));
        assert_eq!(formats.get("m4b").copied(), Some(2));
    }

    #[test]
    fn empty_formats_filter_keeps_all_books() {
        let books = vec![
            with_formats(sample()[0].clone(), &["EPUB"]),
            with_formats(sample()[1].clone(), &["m4b"]),
        ];
        let out = apply_filters(&books, &ViewFilters::default());
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn format_filter_or_within_bucket() {
        let books = vec![
            with_formats(sample()[0].clone(), &["epub"]),
            with_formats(sample()[1].clone(), &["m4b"]),
            with_formats(sample()[2].clone(), &["pdf"]),
        ];
        let f = ViewFilters {
            formats: vec!["epub".into(), "m4b".into()],
            ..Default::default()
        };
        let out = apply_filters(&books, &f);
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn format_filter_matches_case_insensitively() {
        // A book whose persisted format string is upper-case "EPUB" should
        // still match a filter chip whose normalized key is "epub".
        let books = vec![with_formats(sample()[0].clone(), &["EPUB"])];
        let f = ViewFilters {
            formats: vec!["epub".into()],
            ..Default::default()
        };
        let out = apply_filters(&books, &f);
        assert_eq!(ids(&out), vec![1]);
    }

    #[test]
    fn format_filter_intersects_with_other_facets() {
        // Tolkien wrote `alpha` (Fantasy + Foundation) and only that book is
        // EPUB — a Tolkien + EPUB filter should leave just alpha.
        let books = vec![
            with_formats(sample()[0].clone(), &["epub"]),
            with_formats(sample()[1].clone(), &["m4b"]),
        ];
        let f = ViewFilters {
            authors: vec!["Tolkien, J.R.R.".into()],
            formats: vec!["epub".into()],
            ..Default::default()
        };
        let out = apply_filters(&books, &f);
        assert_eq!(ids(&out), vec![1]);
    }

    #[test]
    fn format_display_label_friendly_names() {
        assert_eq!(format_display_label("epub"), "ePub");
        assert_eq!(format_display_label("m4b"), "Audiobook");
        assert_eq!(format_display_label("pdf"), "PDF");
        // Unknown formats fall through upper-cased.
        assert_eq!(format_display_label("azw3"), "AZW3");
    }

    #[test]
    fn short_path_returns_last_segment() {
        assert_eq!(short_path("/Users/ek/books"), "books");
        assert_eq!(short_path("/Users/ek/books/"), "books");
        assert_eq!(short_path("relative"), "relative");
    }
}
