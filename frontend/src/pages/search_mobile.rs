//! Mobile-native search screen — the "Search & discovery" surface.
//!
//! Reached from the Library header's magnifying-glass button. Owns a live,
//! debounced query over the shared [`crate::data::search_palette`] REST call and
//! renders grouped Authors/Books/Series/Tags results per the mobile design.

use dioxus::core::Task;
use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{PaletteBookHit, PaletteResults};

use crate::{data, use_server_url, Route};

/// Which result groups the scope chips let through. Purely client-side —
/// the fetched [`PaletteResults`] is filtered for display, not re-queried.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    Books,
    Authors,
    Series,
    Tags,
}

impl Scope {
    fn shows_books(self) -> bool {
        matches!(self, Scope::All | Scope::Books)
    }
    fn shows_authors(self) -> bool {
        matches!(self, Scope::All | Scope::Authors)
    }
    fn shows_series(self) -> bool {
        matches!(self, Scope::All | Scope::Series)
    }
    fn shows_tags(self) -> bool {
        matches!(self, Scope::All | Scope::Tags)
    }
}

const SCOPES: [(Scope, &str); 5] = [
    (Scope::All, "All"),
    (Scope::Books, "Books"),
    (Scope::Authors, "Authors"),
    (Scope::Series, "Series"),
    (Scope::Tags, "Tags"),
];

/// Mobile live-search screen. Debounces keystrokes (150 ms, cancel-on-keystroke)
/// and renders grouped results; row taps navigate to detail pages, a tag tap
/// refines the query inline.
#[component]
pub fn MobileSearchPage() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();
    let mut query = use_signal(String::new);
    let mut results = use_signal(|| Option::<PaletteResults>::None);
    let mut loading = use_signal(|| false);
    let mut errored = use_signal(|| false);
    let mut scope = use_signal(|| Scope::All);
    // Handle of the in-flight debounce+fetch task so each new keystroke can
    // cancel the prior one — only one request is ever in flight (mirrors the
    // web palette's cancel-on-keystroke debounce).
    let mut current_task = use_signal(|| Option::<Task>::None);

    // Debounced search: re-runs whenever `query` changes.
    let search_url = server_url.clone();
    use_effect(move || {
        let v = query();
        if let Some(prev) = current_task.write().take() {
            prev.cancel();
        }
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            results.set(None);
            loading.set(false);
            errored.set(false);
            return;
        }
        // Flip to loading synchronously — before the debounce sleep — so the
        // results pane shows "Searching…" immediately instead of a blank gap.
        loading.set(true);
        errored.set(false);
        let url = search_url.clone();
        let task = spawn(async move {
            debounce_sleep_ms(150).await;
            match data::search_palette(&url, &trimmed).await {
                Ok(r) => {
                    results.set(Some(r));
                    errored.set(false);
                }
                Err(_) => {
                    results.set(None);
                    errored.set(true);
                }
            }
            loading.set(false);
        });
        current_task.set(Some(task));
    });

    let res = results.read();
    let sc = scope();
    let is_loading = loading();
    let is_errored = errored();
    let q = query();
    let has_query = !q.trim().is_empty();

    rsx! {
        div { class: "m-search", "data-testid": "mobile-search",
            // Search bar: field + Cancel
            div { class: "m-search-bar",
                div { class: "m-search-field",
                    span { class: "m-search-field-icon", {search_glyph()} }
                    input {
                        class: "m-search-input",
                        "data-testid": "mobile-search-input",
                        "aria-label": "Search your library",
                        r#type: "text",
                        placeholder: "Search books, authors, series, tags\u{2026}",
                        autofocus: true,
                        value: "{q}",
                        oninput: move |evt| query.set(evt.value()),
                    }
                }
                button {
                    class: "btn ghost sm m-search-cancel",
                    r#type: "button",
                    "data-testid": "mobile-search-cancel",
                    onclick: move |_| {
                        nav.go_back();
                    },
                    "Cancel"
                }
            }

            // Scope chips
            div { class: "m-search-scopes",
                for (s, label) in SCOPES.iter().copied() {
                    button {
                        key: "{label}",
                        r#type: "button",
                        class: if sc == s { "chip m-search-scope on" } else { "chip m-search-scope" },
                        onclick: move |_| scope.set(s),
                        "{label}"
                    }
                }
            }

            // Results / states
            div { class: "m-search-results", "data-testid": "mobile-search-results",
                if let Some(ref r) = *res {
                    {render_groups(r, sc, &q, server_url.as_str(), query, nav)}
                } else if is_loading {
                    p { class: "m-search-note", "Searching\u{2026}" }
                } else if is_errored {
                    p { class: "m-search-note", role: "alert",
                        "Couldn\u{2019}t run that search. Check your connection and try again."
                    }
                } else if !has_query {
                    p { class: "m-search-note m-search-note-initial",
                        "Search your library by title, author, series or tag."
                    }
                }
            }
        }
    }
}

/// Render the grouped result sections (or the no-results state) for the
/// current scope. Split out to keep [`MobileSearchPage`] readable.
fn render_groups(
    r: &PaletteResults,
    sc: Scope,
    q: &str,
    server_url: &str,
    query: Signal<String>,
    nav: dioxus_router::Navigator,
) -> Element {
    let show_authors = sc.shows_authors() && !r.authors.is_empty();
    let show_books = sc.shows_books() && !r.books.is_empty();
    let show_series = sc.shows_series() && !r.series.is_empty();
    let show_tags = sc.shows_tags() && !r.tags.is_empty();

    if !(show_authors || show_books || show_series || show_tags) {
        return rsx! {
            div { class: "m-search-empty", "data-testid": "mobile-search-empty",
                p { class: "m-search-empty-title", "Nothing for \u{201c}{q}\u{201d}" }
                p { class: "m-search-empty-sub", "We checked titles, authors, series and tags." }
            }
        };
    }

    rsx! {
        if show_authors {
            {group_head("Authors", r.author_total)}
            for a in r.authors.iter() {
                {author_row(a, nav)}
            }
        }
        if show_books {
            {group_head("Books", r.book_total)}
            for b in r.books.iter() {
                {book_row(b, server_url, nav)}
            }
        }
        if show_series {
            {group_head("Series", r.series_total)}
            for s in r.series.iter() {
                {series_row(s, nav)}
            }
        }
        if show_tags {
            {group_head("Tags", r.tag_total)}
            for t in r.tags.iter() {
                {tag_row(t, query)}
            }
        }
    }
}

/// A group heading: label on the left, true match count on the right.
fn group_head(label: &str, count: u32) -> Element {
    rsx! {
        div { class: "m-search-group",
            span { class: "label", "{label}" }
            span { class: "m-search-group-count", "{count}" }
        }
    }
}

/// One author result → author detail page.
fn author_row(a: &omnibus_shared::PaletteAuthorHit, nav: dioxus_router::Navigator) -> Element {
    let id = a.id;
    let key = a.id;
    let avatar = initial(&a.name);
    let name = a.name.clone();
    let sub = format!(
        "Author \u{00b7} {} book{}",
        a.book_count,
        plural(a.book_count)
    );
    rsx! {
        button {
            key: "a{key}",
            r#type: "button",
            class: "m-search-row",
            "data-testid": "mobile-search-author",
            onclick: move |_| { nav.push(Route::AuthorDetail { id }); },
            div { class: "m-search-avatar", "{avatar}" }
            div { class: "m-search-row-body",
                div { class: "m-search-row-title", "{name}" }
                div { class: "m-search-row-sub", "{sub}" }
            }
            span { class: "m-search-row-chevron", {chevron_glyph()} }
        }
    }
}

/// One book result → book detail page.
fn book_row(b: &PaletteBookHit, server_url: &str, nav: dioxus_router::Navigator) -> Element {
    let uuid = b.uuid.clone();
    let key = b.id;
    let title = b.title.clone();
    let author = b.author_display.clone();
    let formats = b.formats.join(" \u{00b7} ");
    rsx! {
        button {
            key: "b{key}",
            r#type: "button",
            class: "m-search-row",
            "data-testid": "mobile-search-book",
            onclick: move |_| { nav.push(Route::BookDetail { uuid: uuid.clone() }); },
            {book_cover(b, server_url)}
            div { class: "m-search-row-body",
                div { class: "m-search-row-title", "{title}" }
                div { class: "m-search-row-sub", "{author}" }
            }
            if !formats.is_empty() {
                span { class: "m-search-row-meta", "{formats}" }
            }
        }
    }
}

/// One series result → series detail page.
fn series_row(s: &omnibus_shared::PaletteSeriesHit, nav: dioxus_router::Navigator) -> Element {
    let id = s.id;
    let key = s.id;
    let name = s.name.clone();
    let sub = match &s.author_display {
        Some(author) => format!(
            "{} book{} \u{00b7} {author}",
            s.book_count,
            plural(s.book_count)
        ),
        None => format!("{} book{}", s.book_count, plural(s.book_count)),
    };
    rsx! {
        button {
            key: "s{key}",
            r#type: "button",
            class: "m-search-row",
            "data-testid": "mobile-search-series",
            onclick: move |_| { nav.push(Route::SeriesDetail { id }); },
            div { class: "m-search-avatar m-search-avatar-series", "\u{224b}" }
            div { class: "m-search-row-body",
                div { class: "m-search-row-title", "{name}" }
                div { class: "m-search-row-sub", "{sub}" }
            }
            span { class: "m-search-row-chevron", {chevron_glyph()} }
        }
    }
}

/// One tag result → refine the query inline to `tag:<name>` (stays on-screen).
fn tag_row(t: &omnibus_shared::PaletteTagHit, mut query: Signal<String>) -> Element {
    let key = t.id;
    let name = t.name.clone();
    let count = format!("{} book{}", t.book_count, plural(t.book_count));
    let facet = tag_facet(&t.name);
    rsx! {
        button {
            key: "t{key}",
            r#type: "button",
            class: "m-search-tag-row",
            "data-testid": "mobile-search-tag",
            onclick: move |_| { query.set(facet.clone()); },
            span { class: "m-search-tag-dot" }
            span { class: "m-search-tag-name", "{name}" }
            span { class: "m-search-tag-count", "{count}" }
        }
    }
}

/// Book cover: thumbnail when present, else an accent-backed initial (mirrors
/// the web palette's `SpBookRow` fallback).
fn book_cover(book: &PaletteBookHit, server_url: &str) -> Element {
    if book.cover_url.is_some() {
        let url = crate::thumb_url(server_url, &book.uuid, "sm");
        return rsx! {
            img { class: "m-search-cover", src: "{url}", alt: "", loading: "lazy" }
        };
    }
    let bg = book.accent.as_deref().unwrap_or("var(--bg-2)").to_string();
    let letter = initial(&book.title);
    rsx! {
        div { class: "m-search-cover m-search-cover-fallback", style: "background: {bg};",
            "{letter}"
        }
    }
}

/// First character of `s`, uppercased, for avatar/fallback glyphs.
fn initial(s: &str) -> String {
    s.chars().next().unwrap_or('?').to_uppercase().to_string()
}

/// Pluralize a book count: `""` for exactly one, `"s"` otherwise.
fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Build a `tag:`-scoped FTS query from a tag name so tapping a tag refines to
/// books carrying it. Each whitespace word is scoped (`tag:Dark tag:academia`)
/// to match how the FTS matcher column-scopes tokens.
fn tag_facet(name: &str) -> String {
    name.split_whitespace()
        .map(|w| format!("tag:{w}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Magnifying-glass icon (inline SVG, `currentColor`) — same glyph as the web
/// palette input.
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

/// Right-chevron affordance for a navigable row.
fn chevron_glyph() -> Element {
    rsx! {
        svg {
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M9 18l6-6-6-6" }
        }
    }
}

// ── Async debounce sleep (mobile: tokio timer) ───────────────────
async fn debounce_sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}
