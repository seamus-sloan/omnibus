//! Grouped results list rendered inside the palette panel — books,
//! authors, series, tags, genres, and the "Inside text" placeholder. Each
//! row component owns its click handler that closes the palette and routes
//! into the matching detail page (or `/search` for tag and genre facets).

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{
    PaletteAuthorHit, PaletteBookHit, PaletteGenreHit, PaletteResults, PaletteSeriesHit,
    PaletteTagHit,
};

use super::model::{facet_query, is_selected, plural, FlatItem};
use super::PaletteOpen;
use crate::{use_server_url, Route};

/// Build a row click handler that navigates to `route` and closes the palette.
fn navigate_and_close(
    nav: dioxus_router::Navigator,
    mut open: PaletteOpen,
    route: Route,
) -> impl FnMut(MouseEvent) + 'static {
    move |_| {
        nav.push(route.clone());
        open.0.set(false);
    }
}

/// Scrollable grouped result list rendered inside the palette panel.
///
/// Owns the per-group heading + row layout. Each click callback closes the
/// palette and navigates to the appropriate detail page via `use_navigator`.
#[component]
pub(super) fn SpResultsList(
    results: Signal<Option<PaletteResults>>,
    flat_items: Memo<Vec<FlatItem>>,
    selected: Signal<usize>,
    has_navigated: Signal<bool>,
    open: PaletteOpen,
) -> Element {
    let nav = use_navigator();
    let res = results.read();
    let items = flat_items.read();
    let sel = selected();
    let has_nav = has_navigated();

    rsx! {
        div { class: "sp-results",
            if let Some(ref r) = *res {
                // Books
                if !r.books.is_empty() {
                    SpGroupHead { label: "Books", count: r.books.len() }
                    for book in r.books.iter() {
                        SpBookRow {
                            key: "{book.id}",
                            book: book.clone(),
                            selected: has_nav && is_selected(&items, sel, &FlatItem::Book { uuid: book.uuid.clone(), title: book.title.clone() }),
                            on_click: navigate_and_close(nav, open, Route::BookDetail { uuid: book.uuid.clone() }),
                        }
                    }
                }

                // Authors
                if !r.authors.is_empty() {
                    SpGroupHead { label: "Authors", count: r.authors.len() }
                    for author in r.authors.iter() {
                        SpAuthorRow {
                            key: "{author.id}",
                            author: author.clone(),
                            selected: has_nav && is_selected(&items, sel, &FlatItem::Author { id: author.id, name: author.name.clone() }),
                            on_click: navigate_and_close(nav, open, Route::AuthorDetail { id: author.id }),
                        }
                    }
                }

                // Series
                if !r.series.is_empty() {
                    SpGroupHead { label: "Series", count: r.series.len() }
                    for s in r.series.iter() {
                        SpSeriesRow {
                            key: "{s.id}",
                            series: s.clone(),
                            selected: has_nav && is_selected(&items, sel, &FlatItem::Series { id: s.id, name: s.name.clone() }),
                            on_click: navigate_and_close(nav, open, Route::SeriesDetail { id: s.id }),
                        }
                    }
                }

                // Tags
                if !r.tags.is_empty() {
                    SpGroupHead { label: "Tags", count: r.tags.len() }
                    for tag in r.tags.iter() {
                        SpTagRow {
                            key: "{tag.id}",
                            tag: tag.clone(),
                            selected: has_nav && is_selected(&items, sel, &FlatItem::Tag { id: tag.id, name: tag.name.clone() }),
                            on_click: navigate_and_close(nav, open, Route::Search { query: facet_query("tag", &tag.name) }),
                        }
                    }
                }

                // Genres
                if !r.genres.is_empty() {
                    SpGroupHead { label: "Genres", count: r.genres.len() }
                    for genre in r.genres.iter() {
                        SpGenreRow {
                            key: "{genre.name}",
                            genre: genre.clone(),
                            selected: has_nav && is_selected(&items, sel, &FlatItem::Genre { name: genre.name.clone() }),
                            on_click: navigate_and_close(nav, open, Route::Search { query: facet_query("genre", &genre.name) }),
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
    // A transient thumb-fetch failure otherwise renders the browser's
    // broken-image icon with no self-heal until a full reload. Tracks the
    // *url* that failed (not just a bool) so a later render with a
    // different url — a mobile token rotation, or (once #832 item 4
    // versions thumb URLs) a regenerated thumb — always gets a fresh
    // chance to load, even though this row's component instance stays
    // mounted (same key) across the change.
    let mut broken_cover_src: Signal<Option<String>> = use_signal(|| None);
    let cover_url = book
        .cover_url
        .is_some()
        .then(|| book_thumb_url(&server_url, &book));
    let cover = if let Some(url) =
        cover_url.filter(|u| broken_cover_src.read().as_deref() != Some(u.as_str()))
    {
        rsx! {
            img {
                class: "sp-row-cover",
                src: "{url}",
                alt: "",
                loading: "lazy",
                onerror: {
                    let url = url.clone();
                    move |_| broken_cover_src.set(Some(url.clone()))
                },
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
fn SpGenreRow(
    genre: PaletteGenreHit,
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
            "data-testid": "sp-genre-row",
            onclick: move |evt| on_click.call(evt),
            span { class: "sp-tag-chip", "{genre.name}" }
            div { class: "sp-row-body",
                div { class: "sp-row-sub",
                    "{genre.book_count} book{plural(genre.book_count as usize)}"
                }
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Build the small-thumbnail URL for a palette book row.
///
/// Extracted so the URL shape — `/api/thumbs/{uuid}/sm` — has a test that
/// can't silently regress to the old `id`-based form.
fn book_thumb_url(server_url: &str, book: &PaletteBookHit) -> String {
    crate::thumb_url(server_url, &book.uuid, "sm")
}

#[cfg(test)]
mod tests {
    use super::super::model::build_flat_items;
    use super::*;

    #[test]
    fn book_thumb_url_uses_uuid_not_id() {
        let book = PaletteBookHit {
            id: 99,
            uuid: "abc-def-uuid".to_string(),
            title: "Test Book".to_string(),
            ..PaletteBookHit::default()
        };
        // On the (non-mobile) test build the URL is relative — same-origin,
        // cookie-authed. Mobile prefixes the server base + `?token=`.
        let url = book_thumb_url("http://localhost:3000", &book);
        assert_eq!(url, "/api/thumbs/abc-def-uuid/sm");
        // Guard: the integer id must never appear in the thumb URL.
        assert!(
            !url.contains("99"),
            "thumb URL must not contain the numeric id"
        );
    }

    #[test]
    fn build_flat_items_is_empty_when_results_are_none() {
        let items = build_flat_items(&None);
        assert!(items.is_empty());
    }
}
