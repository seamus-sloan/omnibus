use dioxus::prelude::*;
use omnibus_shared::{EbookLibrary, SortKey, TagWeight, ViewFilters, ViewMode, ViewPrefs};

use crate::components::chip_editor::{collect_suggestions, SuggestionItem};
use crate::{data, use_search_query, use_server_url, view_prefs};

mod filtering;
mod filters;
mod grid;
mod sorting;
mod table;
mod toolbar;

use filtering::{apply_filters, facet_counts};
use filters::{EmptyFiltered, FilterSidebar, FormatChips};
use grid::BookGrid;
use sorting::{default_dir_for, sort_books, toggle_dir};
use table::BookTable;
use toolbar::Toolbar;

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

    // F5.9-lite: admin-only inline-edit affordances on the power-user
    // table. Non-admins see the existing read-only cells; hiding the
    // affordance is the UX contract — `rpc_save_overrides` enforces the
    // real boundary server-side. Web-only (mobile keeps the read-only
    // landing for now per the F5.9-lite plan; admin can edit via the
    // per-book detail page).
    //
    // Reads from the App-wide `CurrentUser` context — no per-mount
    // `/api/auth/me` round-trip. Reactive: re-runs on boot resolve,
    // fresh login, or observed 401.
    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut is_admin = use_signal(|| false);
    #[cfg(feature = "web")]
    {
        let user_ctx = crate::use_current_user().0;
        use_effect(move || {
            is_admin.set(matches!(user_ctx(), Some(Some(ref u)) if u.is_admin));
        });
    }

    // Suggestion pools for the inline Authors chip editor and the
    // (currently future-reserved) Tags pool. Each item carries the
    // book-count the ChipEditor dropdown renders next to the name —
    // mirrors the fetch done by the F5.1 metadata edit page.
    let mut author_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut tag_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    {
        let url = server_url.clone();
        use_effect(move || {
            // Only admins ever see the dropdown, so skip the round-trip
            // entirely until we know we're admin.
            if !is_admin() {
                return;
            }
            let url = url.clone();
            spawn(async move {
                if let Ok(authors) = data::list_authors(&url).await {
                    let items: Vec<SuggestionItem> = authors
                        .into_iter()
                        .map(|a| SuggestionItem::new(a.name, a.book_count))
                        .collect();
                    author_suggestions.set(collect_suggestions(items));
                }
                if let Ok(tags) = data::get_tag_cloud(&url).await {
                    let items: Vec<SuggestionItem> = tags
                        .into_iter()
                        .map(|t: TagWeight| SuggestionItem::new(t.name, t.count))
                        .collect();
                    tag_suggestions.set(collect_suggestions(items));
                }
            });
        });
    }

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
                Err(e) => error.set(Some(e.to_string())),
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
                                is_admin: is_admin(),
                                author_suggestions,
                                tag_suggestions,
                            }
                        },
                        ViewMode::Grid => rsx! {
                            BookGrid {
                                books: visible_books.clone(),
                                server_url: server_url_for_row.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_returns_last_segment() {
        assert_eq!(short_path("/Users/ek/books"), "books");
        assert_eq!(short_path("/Users/ek/books/"), "books");
        assert_eq!(short_path("relative"), "relative");
    }
}
