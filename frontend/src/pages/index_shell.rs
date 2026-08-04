//! Shared "index list page" shell — the fetch-effect, sort-prefs reconcile,
//! scroll-restore, and loading/error early-return wiring that
//! `authors_index` and `series_index` both need. Each page supplies only
//! its own fetch future and sort-pref reader; everything else (filter/sort
//! text, card layout, breadcrumbs) stays page-owned.

use std::future::Future;

use dioxus::prelude::*;
use omnibus_shared::IndexSort;

use crate::components::{PageError, PageLoading};
use crate::data::DataError;
use crate::scroll_restore::use_scroll_restore;
use crate::Route;

/// Signals an index page needs once its shell has resolved: the fetched
/// rows plus the filter/sort inputs the page's own toolbar drives.
pub(crate) struct IndexPageState<T: 'static> {
    pub items: Signal<Vec<T>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub filter: Signal<String>,
    pub sort: Signal<IndexSort>,
}

/// Runs the fetch-effect (re-run on mount and cache-revalidation bumps),
/// the post-mount sort-prefs reconcile, and the scroll-restore wiring
/// common to every index page. `fetch` builds a fresh list future on each
/// call (so it can re-read `server_url`/generation); `load_sort` reads the
/// page's persisted sort axis. `sort` is seeded to `IndexSort::default()`
/// so SSR and first-hydration markup match (rule 07) — the reconcile only
/// fires post-mount, and the sort toolbar renders only after the fetch
/// resolves, so it can't cause a flash.
pub(crate) fn use_index_page_shell<T, F, Fut>(
    fetch: F,
    load_sort: impl Fn() -> IndexSort + 'static,
) -> IndexPageState<T>
where
    T: 'static,
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Result<Vec<T>, DataError>> + 'static,
{
    let items: Signal<Vec<T>> = use_signal(Vec::new);
    let loading = use_signal(|| true);
    let error: Signal<Option<String>> = use_signal(|| None);
    let filter = use_signal(String::new);
    let mut sort = use_signal(IndexSort::default);

    use_fetch_effect(fetch, items, loading, error);

    use_effect(move || {
        let stored = load_sort();
        if stored != *sort.peek() {
            sort.set(stored);
        }
    });

    let ready = use_memo(move || !loading());
    use_scroll_restore(ready);

    IndexPageState {
        items,
        loading,
        error,
        filter,
        sort,
    }
}

/// Fetches `fetch()`'s list on mount and on every cache-revalidation bump.
fn use_fetch_effect<T, F, Fut>(
    fetch: F,
    mut items: Signal<Vec<T>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
) where
    T: 'static,
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Result<Vec<T>, DataError>> + 'static,
{
    let generation = crate::use_cache_generation();
    use_effect(move || {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let fut = fetch();
        spawn(async move {
            if items.peek().is_empty() {
                loading.set(true);
            }
            match fut.await {
                Ok(v) => {
                    items.set(v);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });
}

/// The shared loading/error early-return: `Some(element)` when the caller
/// should return it immediately, `None` once the page should render its
/// own body.
pub(crate) fn index_page_early_return(
    loading: Signal<bool>,
    error: Signal<Option<String>>,
) -> Option<Element> {
    if loading() {
        return Some(rsx! { PageLoading {} });
    }
    if let Some(msg) = error() {
        return Some(rsx! { PageError { message: msg, back_to: Route::Landing {} } });
    }
    None
}

/// The "Books" stat block inside an index card — identical between the
/// series and author card kinds.
pub(crate) fn index_card_stats(count: usize) -> Element {
    rsx! {
        div { class: "idx-card-stats",
            div { class: "idx-stat",
                div { class: "mono idx-stat-label", "Books" }
                div { class: "idx-stat-value", "{count}" }
            }
        }
    }
}

/// Search input for an index page's toolbar, shared by the series and
/// authors index headers. `placeholder`/`aria_label` are the input's hint
/// text; `testid` and `on_filter` are per-page.
#[component]
pub(crate) fn IndexFilterInput(
    filter: String,
    placeholder: String,
    aria_label: String,
    testid: String,
    on_filter: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "idx-search",
            input {
                r#type: "search",
                placeholder: "{placeholder}",
                aria_label: "{aria_label}",
                value: "{filter}",
                "data-testid": "{testid}",
                oninput: move |e| on_filter.call(e.value()),
            }
        }
    }
}

/// Name / most-books two-button sort toggle shared by the series and
/// authors index toolbars. `name_label` differs per page ("A\u{2013}Z" vs
/// "Last name A\u{2013}Z"); testids derive from `testid_prefix`.
#[component]
pub(crate) fn IndexSortToggle(
    sort: IndexSort,
    name_label: String,
    testid_prefix: String,
    on_sort: EventHandler<IndexSort>,
) -> Element {
    rsx! {
        div { class: "idx-sort",
            span { class: "label", "Sort" }
            button {
                class: "idx-btn",
                "aria-pressed": if sort == IndexSort::Name { "true" } else { "false" },
                "data-testid": "{testid_prefix}-sort-name",
                onclick: move |_| on_sort.call(IndexSort::Name),
                "{name_label}"
            }
            button {
                class: "idx-btn",
                "aria-pressed": if sort == IndexSort::BookCount { "true" } else { "false" },
                "data-testid": "{testid_prefix}-sort-count",
                onclick: move |_| on_sort.call(IndexSort::BookCount),
                "Most books"
            }
        }
    }
}
