//! Shared "pick books from the whole library" surface: a searchable card grid
//! where every book carries its title, author and format, and books the shelf
//! already holds are marked and unselectable. Used by the create-shelf modal's
//! hand-picked body and the shelf page's "Add books" modal.
//!
//! The grid is server-backed, mirroring the landing grid's split: an empty
//! query browses keyset pages that append as you scroll, and a typed query
//! goes to FTS5 through [`crate::data::search_ebooks`]. Nothing is ever
//! dropped once fetched — only the fetch is bounded.

use std::collections::HashSet;

use dioxus::core::Task;
use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, SortDir, SortKey, ViewFilters};

use crate::components::atrium::{fallback_title, Cover};
use crate::components::cover_tile::thumb_srcs;
use crate::contexts::{cover_bust_for, CoverCacheBust};
use crate::data;
use crate::focus_after_paint::focus_after_paint;
use crate::platform_sleep::async_sleep_ms;

/// One keyset page, matching the landing grid's page size — the picker scrolls
/// the same library through the same endpoint, so a different size here would
/// only make the two surfaces disagree about how much a "page" is.
const PAGE_SIZE: i64 = 100;

/// How long a typed query sits still before it reaches FTS5. Matches the
/// command palette, which searches the same index.
const DEBOUNCE_MS: u32 = 150;

/// The picker's server feed: the rows it has, how to ask for more, and the
/// lifecycle flags the grid renders from. `Copy` (Dioxus signals), so it is
/// passed by value into effects and helpers.
#[derive(Clone, Copy)]
struct PickerFeed {
    books: Signal<Vec<EbookMetadata>>,
    /// `Some` only while browsing — a search returns one capped result set
    /// with no cursor, so there is nothing to page through.
    next_cursor: Signal<Option<String>>,
    /// The server's own count: library size when browsing, true FTS hit count
    /// when searching. `None` when the server didn't say.
    total: Signal<Option<i64>>,
    loading: Signal<bool>,
    loading_more: Signal<bool>,
    errored: Signal<bool>,
    /// Bumped on every query change; an in-flight fetch whose epoch is stale
    /// drops its result rather than splicing it onto a newer list.
    epoch: Signal<u32>,
    /// Handle of the debounce+fetch task, cancelled on the next keystroke.
    task: Signal<Option<Task>>,
    /// Bumped to ask for the next page.
    want_more: Signal<u32>,
}

/// Wire the picker's two fetch effects: the query-driven page-1/search fetch,
/// and the append-the-next-page fetch. Returns the feed the grid reads.
fn use_picker_feed(server_url: String, query: Signal<String>) -> PickerFeed {
    let feed = PickerFeed {
        books: use_signal(Vec::<EbookMetadata>::new),
        next_cursor: use_signal(|| None::<String>),
        total: use_signal(|| None::<i64>),
        loading: use_signal(|| true),
        loading_more: use_signal(|| false),
        errored: use_signal(|| false),
        epoch: use_signal(|| 0u32),
        task: use_signal(|| None::<Task>),
        want_more: use_signal(|| 0u32),
    };
    use_query_effect(server_url.clone(), query, feed);
    use_load_more_effect(server_url, feed);
    use_load_more_observer(feed.next_cursor);
    feed
}

/// Refetch from scratch whenever the query settles: keyset page 1 when it is
/// empty, FTS5 otherwise.
fn use_query_effect(server_url: String, query: Signal<String>, feed: PickerFeed) {
    let PickerFeed {
        mut loading,
        mut errored,
        mut epoch,
        mut task,
        ..
    } = feed;
    use_effect(move || {
        let q = query();
        // One task at a time: a keystroke cancels the pending sleep and any
        // request it had already issued.
        if let Some(prev) = task.write().take() {
            prev.cancel();
        }
        let mine = {
            epoch.with_mut(|e| *e += 1);
            *epoch.peek()
        };
        // Flip to loading synchronously so the grid shows a state rather than
        // a blank gap while the debounce runs.
        loading.set(true);
        errored.set(false);
        let url = server_url.clone();
        let handle = spawn(async move {
            let trimmed = q.trim().to_string();
            // Only a typed query pays the debounce — clearing the box should
            // put the library back immediately.
            if !trimmed.is_empty() {
                async_sleep_ms(DEBOUNCE_MS).await;
            }
            if trimmed.is_empty() {
                let result = browse_page(&url, None).await;
                if *epoch.peek() == mine {
                    apply_browse(feed, result);
                }
            } else {
                let result = data::search_ebooks(&url, &trimmed).await;
                if *epoch.peek() == mine {
                    apply_search(feed, result);
                }
            }
            if *epoch.peek() == mine {
                loading.set(false);
            }
        });
        task.set(Some(handle));
    });
}

/// Append the next keyset page when `want_more` bumps. Browsing only — a
/// search has no cursor.
fn use_load_more_effect(server_url: String, feed: PickerFeed) {
    let PickerFeed {
        mut books,
        mut next_cursor,
        mut loading_more,
        mut errored,
        epoch,
        want_more,
        ..
    } = feed;
    use_effect(move || {
        let trigger = want_more();
        if trigger == 0 || *loading_more.peek() || next_cursor.peek().is_none() {
            return;
        }
        let cursor = next_cursor.peek().clone();
        let mine = *epoch.peek();
        let url = server_url.clone();
        loading_more.set(true);
        spawn(async move {
            let result = browse_page(&url, cursor).await;
            // Drop the append if a new query superseded us mid-flight —
            // otherwise an old result stream is spliced onto the new list and
            // overwrites its cursor.
            if *epoch.peek() != mine {
                loading_more.set(false);
                return;
            }
            match result {
                Ok(page) => {
                    books.with_mut(|b| b.extend(page.books));
                    next_cursor.set(page.next_cursor);
                }
                Err(_) => errored.set(true),
            }
            loading_more.set(false);
        });
    });
}

/// One browse page. The picker browses title-ascending with no facet filters:
/// it is a find-this-book surface, not the reader's configured library view.
async fn browse_page(
    server_url: &str,
    cursor: Option<String>,
) -> Result<omnibus_shared::LibraryPage, data::DataError> {
    data::get_ebooks_page(
        server_url,
        SortKey::Title,
        SortDir::Asc,
        ViewFilters::default(),
        Vec::new(),
        cursor,
        PAGE_SIZE,
    )
    .await
}

/// Apply a browse page-1 result.
fn apply_browse(feed: PickerFeed, result: Result<omnibus_shared::LibraryPage, data::DataError>) {
    let PickerFeed {
        mut books,
        mut next_cursor,
        mut total,
        mut errored,
        ..
    } = feed;
    match result {
        Ok(page) => {
            next_cursor.set(page.next_cursor);
            total.set(page.total);
            books.set(page.books);
            errored.set(false);
        }
        Err(_) => {
            books.set(Vec::new());
            next_cursor.set(None);
            total.set(None);
            errored.set(true);
        }
    }
}

/// Apply a search result: one capped set, no cursor, `total` carrying the true
/// hit count from the single FTS5 pass.
fn apply_search(feed: PickerFeed, result: Result<omnibus_shared::EbookLibrary, data::DataError>) {
    let PickerFeed {
        mut books,
        mut next_cursor,
        mut total,
        mut errored,
        ..
    } = feed;
    next_cursor.set(None);
    match result {
        Ok(lib) => {
            total.set(lib.total);
            books.set(lib.books);
            errored.set(false);
        }
        Err(_) => {
            books.set(Vec::new());
            total.set(None);
            errored.set(true);
        }
    }
}

/// Auto-bump the load-more sentinel as it nears the picker's scroll area.
///
/// The hook is declared unconditionally on every target and only its *body*
/// is gated, so SSR and the first WASM render agree on hook order (rule 07).
/// The observer is keyed to its own global and testid rather than sharing the
/// landing grid's — two observers on one handle would disconnect each other.
fn use_load_more_observer(next_cursor: Signal<Option<String>>) {
    use_effect(move || {
        // Re-arm after each append: the sentinel is replaced, so the old
        // observation is stale.
        let _rearm_on = next_cursor.read().is_some();
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval(
                r#"
                if (window.__omnibusPickerMoreObs) {
                    window.__omnibusPickerMoreObs.disconnect();
                    window.__omnibusPickerMoreObs = null;
                }
                const el = document.querySelector('[data-testid="picker-load-more"]');
                if (el) {
                    const obs = new IntersectionObserver((entries) => {
                        if (entries.some((e) => e.isIntersecting)) { el.click(); }
                    }, { root: document.querySelector('.pick-body'), rootMargin: "300px" });
                    obs.observe(el);
                    window.__omnibusPickerMoreObs = obs;
                }
                "#,
            );
        }
    });
}

/// `uuid` dropped when `list` already holds it, appended otherwise. Split out
/// so the rule is testable without a Dioxus runtime (`Signal::new` panics
/// outside one).
fn toggled(list: &[String], uuid: &str) -> Vec<String> {
    let mut next: Vec<String> = list
        .iter()
        .filter(|x| x.as_str() != uuid)
        .cloned()
        .collect();
    if next.len() == list.len() {
        next.push(uuid.to_string());
    }
    next
}

/// The same rule over the picked books' metadata, which the picker keeps so
/// the "Picked" view can render a book that the current result set no longer
/// contains — pick it under one search, review it under another.
fn toggled_meta(list: &[EbookMetadata], book: &EbookMetadata, uuid: &str) -> Vec<EbookMetadata> {
    let mut next: Vec<EbookMetadata> = list
        .iter()
        .filter(|b| b.unique_identifier.as_deref() != Some(uuid))
        .cloned()
        .collect();
    if next.len() == list.len() {
        next.push(book.clone());
    }
    next
}

/// The line above the grid: how much the server holds.
///
/// Browsing pages until every book is loaded, so how many are on screen at
/// this instant is noise — scrolling reaches all of them — and the line simply
/// says how many there are. A search is the one case that withholds rows: it
/// is capped server-side and carries no cursor, so the shortfall is real and
/// worth naming. The count of what's *picked* belongs to the host modal's
/// submit button, which is the one place it's reported.
fn status_line(searching: bool, shown: usize, total: Option<i64>) -> String {
    let (one, many) = if searching {
        ("match", "matches")
    } else {
        ("book", "books")
    };
    let Some(t) = total.map(|t| t.max(0) as usize) else {
        // The server didn't say — report only what we can see rather than
        // implying the grid is the whole library.
        return plural(shown, one, many);
    };
    if !searching || t <= shown {
        return plural(t, one, many);
    }
    format!(
        "Showing {shown} of {} \u{b7} narrow your search",
        plural(t, one, many)
    )
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The picker. `already` are the uuids the target shelf holds, drawn as
/// members rather than choices; `search_testid` names the search box for its
/// host modal; `autofocus` puts the caret in it when the modal opens.
#[component]
pub fn LibraryPicker(
    server_url: String,
    picked: Signal<Vec<String>>,
    #[props(default)] already: Vec<String>,
    search_testid: String,
    #[props(default)] autofocus: bool,
) -> Element {
    let query = use_signal(String::new);
    let mut show_picked = use_signal(|| false);
    let picked_meta = use_signal(Vec::<EbookMetadata>::new);
    let bust = use_context::<CoverCacheBust>();
    let feed = use_picker_feed(server_url.clone(), query);

    let text = query();
    let searching = !text.trim().is_empty();
    let reviewing = show_picked();
    let picked_now: HashSet<String> = picked.read().iter().cloned().collect();
    let pick_count = picked_now.len();
    // Reviewing picks is a view over what you chose, not over the result set,
    // so it renders from the metadata cache instead of the feed.
    let rows: Vec<EbookMetadata> = if reviewing {
        picked_meta.read().clone()
    } else {
        feed.books.read().clone()
    };
    let status = if reviewing {
        plural(pick_count, "book picked", "books picked")
    } else {
        status_line(searching, rows.len(), feed.total.read().as_ref().copied())
    };
    let ctx = CardCtx {
        server_url,
        already: already.into_iter().collect(),
        picked_now,
        picked,
        picked_meta,
        bust,
    };

    rsx! {
        div { class: "pick",
            div { class: "pick-search-row",
                {search_box(query, &text, &search_testid, autofocus)}
                if pick_count > 0 {
                    button {
                        r#type: "button",
                        class: "pick-review",
                        "aria-pressed": if reviewing { "true" } else { "false" },
                        "data-testid": "picker-review-picked",
                        onclick: move |_| show_picked.with_mut(|v| *v = !*v),
                        "Picked ({pick_count})"
                    }
                }
                p { class: "pick-status", role: "status", "data-testid": "picker-status",
                    "{status}"
                }
            }
            div { class: "pick-body",
                {body(&rows, BodyState {
                    loading: feed.loading.read().to_owned(),
                    errored: feed.errored.read().to_owned(),
                    searching,
                    reviewing,
                    query: text.trim().to_string(),
                }, &ctx)}
                {load_more(feed, reviewing)}
            }
        }
    }
}

/// The search input and its clear button.
fn search_box(
    mut query: Signal<String>,
    text: &str,
    search_testid: &str,
    autofocus: bool,
) -> Element {
    rsx! {
        div { class: "pick-search",
            {search_icon()}
            input {
                r#type: "search",
                placeholder: "Search by title or author\u{2026}",
                "aria-label": "Search your library",
                "data-testid": "{search_testid}",
                value: "{text}",
                oninput: move |e| query.set(e.value()),
                onmounted: move |evt: MountedEvent| {
                    if autofocus {
                        focus_after_paint(&evt);
                    }
                },
            }
            if !text.is_empty() {
                button {
                    r#type: "button",
                    class: "pick-search-clear",
                    "aria-label": "Clear search",
                    "data-testid": "picker-clear-search",
                    onclick: move |_| query.set(String::new()),
                    {x_icon()}
                }
            }
        }
    }
}

/// The load-more sentinel. A real button so mobile and Playwright have a
/// deterministic trigger; on web the observer clicks it as it nears view.
fn load_more(feed: PickerFeed, reviewing: bool) -> Element {
    let mut want_more = feed.want_more;
    let has_more = feed.next_cursor.read().is_some();
    let busy = feed.loading_more.read().to_owned();
    if reviewing || !has_more {
        return rsx! {};
    }
    rsx! {
        div { class: "pick-more-row",
            button {
                r#type: "button",
                class: "btn pick-more",
                "data-testid": "picker-load-more",
                disabled: busy,
                onclick: move |_| want_more.with_mut(|n| *n += 1),
                if busy { "Loading\u{2026}" } else { "Load more" }
            }
        }
    }
}

/// Everything a card needs beyond its book. Bundled so [`card`] stays inside
/// clippy's argument cap.
struct CardCtx {
    server_url: String,
    /// A set, not a list: this is consulted once per rendered card, and a
    /// shelf can hold thousands of books.
    already: HashSet<String>,
    picked_now: HashSet<String>,
    picked: Signal<Vec<String>>,
    picked_meta: Signal<Vec<EbookMetadata>>,
    bust: CoverCacheBust,
}

/// Which non-grid state the body may be in.
struct BodyState {
    loading: bool,
    errored: bool,
    searching: bool,
    reviewing: bool,
    query: String,
}

/// The grid, or the state that says why there isn't one.
fn body(rows: &[EbookMetadata], state: BodyState, ctx: &CardCtx) -> Element {
    if state.loading && rows.is_empty() {
        return rsx! {
            p { class: "pick-state", "data-testid": "picker-loading",
                "Loading your library\u{2026}"
            }
        };
    }
    if state.errored && rows.is_empty() {
        return rsx! {
            p { role: "alert", class: "pick-state", "data-testid": "picker-error",
                "Couldn\u{2019}t reach your library. Check your connection and try again."
            }
        };
    }
    if rows.is_empty() {
        return rsx! {
            p { class: "pick-state", "data-testid": "picker-empty",
                {empty_message(&state)}
            }
        };
    }
    rsx! {
        div { class: "pick-grid",
            for book in rows.iter() {
                {card(book, ctx)}
            }
        }
    }
}

/// Why the grid is empty, in the reader's terms.
fn empty_message(state: &BodyState) -> String {
    if state.reviewing {
        "Nothing picked yet.".to_string()
    } else if state.searching {
        format!("No books match \u{201c}{}\u{201d}.", state.query)
    } else {
        "No books in your library yet.".to_string()
    }
}

/// One book as a pickable card: cover, title, author, format — and, when the
/// shelf already holds it, a note saying so in place of the check.
fn card(book: &EbookMetadata, ctx: &CardCtx) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let title = fallback_title(book.title.as_deref(), &book.filename);
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let format = book.formats.first().cloned().unwrap_or_default();
    let on_shelf = ctx.already.contains(&uuid);
    let selected = ctx.picked_now.contains(&uuid);
    let (src, srcset) = thumb_srcs(
        book,
        &uuid,
        &ctx.server_url,
        cover_bust_for(ctx.bust.0, &uuid),
    );
    let mut picked = ctx.picked;
    let mut picked_meta = ctx.picked_meta;
    let pick_uuid = uuid.clone();
    let pick_book = book.clone();
    rsx! {
        button {
            key: "{uuid}",
            r#type: "button",
            class: "pick-card",
            "data-testid": "picker-tile-{uuid}",
            "aria-pressed": if selected { "true" } else { "false" },
            "aria-disabled": if on_shelf { "true" } else { "false" },
            onclick: move |_| {
                if on_shelf {
                    return;
                }
                picked.with_mut(|v| *v = toggled(v, &pick_uuid));
                picked_meta.with_mut(|v| *v = toggled_meta(v, &pick_book, &pick_uuid));
            },
            span { class: "pick-cover",
                Cover {
                    book: book.clone(),
                    src_override: src,
                    srcset,
                    sizes: Some("44px".to_string()),
                }
            }
            span { class: "pick-meta",
                span { class: "pick-name", "{title}" }
                if !author.is_empty() {
                    span { class: "pick-author", "{author}" }
                }
                span { class: "pick-tags",
                    if on_shelf {
                        span { class: "pick-on-shelf", "On this shelf" }
                    } else if !format.is_empty() {
                        span { class: "pick-tag", "{format}" }
                    }
                }
            }
            span { class: "pick-check", aria_hidden: true,
                if selected {
                    {check_icon()}
                }
            }
        }
    }
}

fn search_icon() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}

fn x_icon() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2.2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M18 6 6 18M6 6l12 12" }
        }
    }
}

fn check_icon() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "3",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M20 6 9 17l-5-5" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(title: &str, uuid: &str) -> EbookMetadata {
        EbookMetadata {
            title: Some(title.to_string()),
            unique_identifier: Some(uuid.to_string()),
            filename: format!("{uuid}.epub"),
            ..Default::default()
        }
    }

    #[test]
    fn toggled_appends_a_new_pick_and_drops_an_existing_one() {
        let picked = toggled(&[], "a");
        assert_eq!(picked, vec!["a".to_string()]);
        let picked = toggled(&picked, "b");
        assert_eq!(picked, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(toggled(&picked, "a"), vec!["b".to_string()]);
    }

    #[test]
    fn toggled_meta_keeps_the_picked_books_in_step_with_their_uuids() {
        let a = book("Alpha", "a");
        let b = book("Beta", "b");
        let list = toggled_meta(&[], &a, "a");
        assert_eq!(list.len(), 1);
        let list = toggled_meta(&list, &b, "b");
        assert_eq!(list.len(), 2);
        let list = toggled_meta(&list, &a, "a");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title.as_deref(), Some("Beta"));
    }

    #[test]
    fn status_line_reports_the_library_size_when_everything_is_loaded() {
        assert_eq!(status_line(false, 42, Some(42)), "42 books");
        assert_eq!(status_line(false, 1, Some(1)), "1 book");
    }

    #[test]
    fn status_line_reports_the_whole_library_while_browsing_a_partial_page() {
        // Only 100 rows are loaded, but paging reaches every one of the rest,
        // so the line says how many there are rather than how many happen to
        // be on screen at this instant.
        assert_eq!(status_line(false, 100, Some(2310)), "2310 books");
    }

    #[test]
    fn status_line_tells_a_truncated_search_to_narrow() {
        // A search is capped server-side and carries no cursor, so scrolling
        // cannot reach the rest.
        assert_eq!(
            status_line(true, 50, Some(2310)),
            "Showing 50 of 2310 matches \u{b7} narrow your search"
        );
        assert_eq!(status_line(true, 3, Some(3)), "3 matches");
    }

    #[test]
    fn status_line_reports_only_what_it_can_see_when_the_server_sent_no_total() {
        // "Can't tell" must not render as "this is the whole library".
        assert_eq!(status_line(false, 12, None), "12 books");
    }

    #[test]
    fn empty_message_distinguishes_no_picks_from_no_matches() {
        let state = |searching, reviewing, q: &str| BodyState {
            loading: false,
            errored: false,
            searching,
            reviewing,
            query: q.to_string(),
        };
        assert_eq!(
            empty_message(&state(false, true, "")),
            "Nothing picked yet."
        );
        assert_eq!(
            empty_message(&state(true, false, "zzz")),
            "No books match \u{201c}zzz\u{201d}."
        );
        assert_eq!(
            empty_message(&state(false, false, "")),
            "No books in your library yet."
        );
    }
}
