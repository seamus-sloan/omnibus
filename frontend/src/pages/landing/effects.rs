//! Data-fetch + side-effect helpers spawned from [`super::LandingPage`].
//! Each function captures the signals it mutates and registers the matching
//! `use_effect` so the page body stays a small composition of named stages.

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, TagWeight, ViewPrefs};

use crate::components::chip_editor::{collect_suggestions, SuggestionItem};
use crate::data;

use super::PAGE_SIZE;

/// Stable per-page handle on the chip-editor suggestion pool signals (authors, tags).
#[derive(Copy, Clone)]
pub(super) struct SuggestionPools {
    pub(super) authors: Signal<Vec<SuggestionItem>>,
    pub(super) tags: Signal<Vec<SuggestionItem>>,
}

/// Bundle of fetch-target signals + epoch counter passed into [`spawn_page_fetch_effect`].
#[derive(Copy, Clone)]
pub(super) struct FetchSignals {
    pub(super) books: Signal<Vec<EbookMetadata>>,
    pub(super) next_cursor: Signal<Option<String>>,
    pub(super) total: Signal<Option<i64>>,
    pub(super) lib_path: Signal<Option<String>>,
    pub(super) lib_error: Signal<Option<String>>,
    pub(super) loading: Signal<bool>,
    pub(super) loading_more: Signal<bool>,
    pub(super) error: Signal<Option<String>>,
    pub(super) fetch_epoch: Signal<u64>,
}

/// Refetch the admin-only author/tag suggestion pools whenever `is_admin` changes.
pub(super) fn spawn_suggestion_pools_effect(
    server_url: String,
    is_admin: Signal<bool>,
    pools: SuggestionPools,
) {
    let SuggestionPools {
        mut authors,
        mut tags,
    } = pools;
    use_effect(move || {
        if !is_admin() {
            return;
        }
        let url = server_url.clone();
        spawn(async move {
            if let Ok(list) = data::list_authors(&url).await {
                let items: Vec<SuggestionItem> = list
                    .into_iter()
                    .map(|a| SuggestionItem::new(a.name, a.book_count))
                    .collect();
                authors.set(collect_suggestions(items));
            }
            if let Ok(list) = data::get_tag_cloud(&url).await {
                let items: Vec<SuggestionItem> = list
                    .into_iter()
                    .map(|t: TagWeight| SuggestionItem::new(t.name, t.count))
                    .collect();
                tags.set(collect_suggestions(items));
            }
        });
    });
}

/// Refetch page 1 on query/sort/filter changes; epoch-guarded so stale in-flight requests drop.
pub(super) fn spawn_page_fetch_effect(
    server_url: String,
    fetch_key: Memo<(
        String,
        omnibus_shared::SortKey,
        omnibus_shared::SortDir,
        omnibus_shared::ViewFilters,
    )>,
    sigs: FetchSignals,
) {
    // `sigs` is `Copy`; the result-application signals are set inside
    // `apply_browse_result` / `apply_search_result`. Only the fetch-lifecycle
    // signals are driven from this body.
    let mut next_cursor = sigs.next_cursor;
    let mut loading = sigs.loading;
    let mut error = sigs.error;
    let mut fetch_epoch = sigs.fetch_epoch;
    use_effect(move || {
        let (q, sort_key, sort_dir, filters) = fetch_key();
        let epoch = {
            fetch_epoch.with_mut(|e| *e += 1);
            *fetch_epoch.peek()
        };
        let url = server_url.clone();
        spawn(async move {
            loading.set(true);
            error.set(None);
            next_cursor.set(None);
            if q.is_empty() {
                // Browse: server keyset page 1 (server-side sort + filter,
                // facets + total ride along on the first page).
                let result =
                    data::get_ebooks_page(&url, sort_key, sort_dir, filters, None, PAGE_SIZE).await;
                if *fetch_epoch.peek() != epoch {
                    return; // a newer fetch superseded us — drop this result
                }
                apply_browse_result(sigs, result);
            } else {
                // Search: capped full result set, sorted/filtered client-side.
                let result = data::search_ebooks(&url, &q).await;
                if *fetch_epoch.peek() != epoch {
                    return;
                }
                apply_search_result(sigs, result);
            }
            loading.set(false);
        });
    });
}

/// Apply a browse (keyset page-1) fetch outcome to the page signals. On error,
/// the derived signals are cleared so the error state doesn't render a stale
/// header count.
fn apply_browse_result(
    sigs: FetchSignals,
    result: Result<omnibus_shared::LibraryPage, data::DataError>,
) {
    let FetchSignals {
        mut books,
        mut next_cursor,
        mut total,
        mut lib_path,
        mut lib_error,
        mut error,
        ..
    } = sigs;
    match result {
        Ok(page) => {
            lib_path.set(page.path);
            lib_error.set(None);
            next_cursor.set(page.next_cursor);
            total.set(page.total);
            books.set(page.books);
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            books.set(Vec::new());
            total.set(None);
            lib_error.set(None);
        }
    }
}

/// Apply a search (capped full result set) fetch outcome to the page signals.
/// On error, the derived signals are cleared to avoid a stale header count.
fn apply_search_result(
    sigs: FetchSignals,
    result: Result<omnibus_shared::EbookLibrary, data::DataError>,
) {
    let FetchSignals {
        mut books,
        mut total,
        mut lib_path,
        mut lib_error,
        mut error,
        ..
    } = sigs;
    match result {
        Ok(lib) => {
            lib_path.set(lib.path);
            lib_error.set(lib.error);
            total.set(lib.total);
            books.set(lib.books);
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            books.set(Vec::new());
            total.set(None);
            lib_error.set(None);
        }
    }
}

/// Append the next page when `want_more` bumps; drops the append if a page-1 refetch supersedes it.
pub(super) fn spawn_load_more_effect(
    server_url: String,
    want_more: Signal<u32>,
    prefs: Signal<ViewPrefs>,
    sigs: FetchSignals,
) {
    let FetchSignals {
        mut books,
        mut next_cursor,
        mut loading_more,
        mut error,
        fetch_epoch,
        ..
    } = sigs;
    use_effect(move || {
        let trigger = want_more();
        if trigger == 0 || *loading_more.peek() || next_cursor.peek().is_none() {
            return;
        }
        let p = prefs.peek().clone();
        let cursor = next_cursor.peek().clone();
        let epoch = *fetch_epoch.peek();
        let url = server_url.clone();
        loading_more.set(true);
        spawn(async move {
            let result =
                data::get_ebooks_page(&url, p.sort_key, p.sort_dir, p.filters, cursor, PAGE_SIZE)
                    .await;
            // Drop the append if a page-1 refetch (sort/filter/query change)
            // superseded us mid-flight — otherwise we'd splice an old result
            // stream onto the new list and overwrite its cursor.
            if *fetch_epoch.peek() != epoch {
                loading_more.set(false);
                return;
            }
            match result {
                Ok(page) => {
                    books.with_mut(|b| b.extend(page.books));
                    next_cursor.set(page.next_cursor);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading_more.set(false);
        });
    });
}

/// Web-only: arm an `IntersectionObserver` on the load-more sentinel; re-arms after each page append.
#[cfg(feature = "web")]
pub(super) fn spawn_load_more_observer(next_cursor: Signal<Option<String>>) {
    use_effect(move || {
        let _rearm_on = next_cursor.read().is_some();
        let _ = dioxus::document::eval(
            r#"
            if (window.__omnibusLoadMoreObs) {
                window.__omnibusLoadMoreObs.disconnect();
                window.__omnibusLoadMoreObs = null;
            }
            const el = document.querySelector('[data-testid="lib-load-more"]');
            if (el) {
                const obs = new IntersectionObserver((entries) => {
                    if (entries.some((e) => e.isIntersecting)) { el.click(); }
                }, { rootMargin: "400px" });
                obs.observe(el);
                window.__omnibusLoadMoreObs = obs;
            }
            "#,
        );
    });
}
