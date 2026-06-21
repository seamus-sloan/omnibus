//! Admin-only "Merge with…" dialog mounted by the book detail page:
//! search-and-confirm flow that picks another book via FTS and calls
//! `data::merge_books` to absorb it into the page's book (the current
//! book is always the merge **target**; its metadata wins). Parent
//! receives the [`MergeBooksResult`] for its refetch + undo toast.

use dioxus::core::Task;
use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, MergeBooksResult};

use crate::components::atrium::Cover;
use crate::{data, use_server_url};

/// Modal dialog: target rail → search input → candidate rows → confirm
/// panel. The current book (`target`) is always the merge keeper; every
/// duplicate folds into it.
#[component]
pub fn MergeDialog(
    target: EbookMetadata,
    on_merged: EventHandler<MergeBooksResult>,
    on_close: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let target_uuid = target.unique_identifier.clone().unwrap_or_default();
    let target_title = target
        .title
        .clone()
        .unwrap_or_else(|| target.filename.clone());

    let mut query = use_signal(String::new);
    let mut results: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    let mut selected: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    // Handle of the in-flight debounce+RPC task so the next keystroke can
    // `.cancel()` it before spawning a new one — only one search is ever
    // in flight (same pattern as search_palette/overlay.rs, #126).
    let mut current_task = use_signal(|| Option::<Task>::None);

    let search_url = server_url.clone();
    let search_exclude = target_uuid.clone();
    let mut run_search = move |q: String| {
        if let Some(prev) = current_task.write().take() {
            prev.cancel();
        }
        let url = search_url.clone();
        let exclude = search_exclude.clone();
        let task = spawn(async move {
            async_sleep_ms(150).await;
            if q.trim().is_empty() {
                results.set(Vec::new());
                error.set(None);
                return;
            }
            // The cancel above makes stale completions unlikely, but a
            // response already past the await when the next keystroke
            // lands could still race — drop anything (success *or*
            // failure) that no longer matches what the user typed.
            // `merge_candidates` (not `search_ebooks`): the candidate
            // pool must span both libraries — the typical merge pairs
            // an ebook with an audiobook.
            match data::merge_candidates(&url, &q).await {
                Ok(books) => {
                    if query() == q {
                        error.set(None);
                        results.set(
                            books
                                .into_iter()
                                .filter(|b| {
                                    b.unique_identifier.as_deref() != Some(exclude.as_str())
                                })
                                .take(8)
                                .collect(),
                        );
                    }
                }
                Err(e) => {
                    if query() == q {
                        error.set(Some(e.to_string()));
                    }
                }
            }
        });
        current_task.set(Some(task));
    };

    let merge_url = server_url.clone();
    let merge_target = target_uuid.clone();
    let do_merge = move |_| {
        // Re-entrancy guard: a double-click could land before the
        // disabled state re-renders.
        if busy() {
            return;
        }
        let Some(source) = selected() else { return };
        let Some(source_uuid) = source.unique_identifier.clone() else {
            return;
        };
        let url = merge_url.clone();
        let target = merge_target.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::merge_books(&url, &source_uuid, &target).await {
                Ok(res) => on_merged.call(res),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    let target_series = target.series.clone();
    let result_count = results().len();

    rsx! {
        div {
            class: "author-photo-modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            aria_label: "Merge with another book",
            // No dismissal while a merge is executing — closing would
            // hide the outcome (the request itself keeps running).
            onclick: move |_| {
                if !busy() {
                    on_close.call(());
                }
            },
            div {
                class: "mg-modal",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "mg-head",
                    div { class: "label mg-kicker", "Merge duplicate" }
                    h2 { class: "mg-title", "Merge with\u{2026}" }
                }
                div { class: "mg-body",
                    if let Some(src) = selected() {
                        {render_confirm(src, target_title.clone(), busy(), do_merge, move |_| selected.set(None))}
                    } else {
                        p { class: "mg-intro",
                            "Find the other copy of this book. Its files, tags, highlights and reading "
                            "progress move onto the entry you\u{2019}re viewing \u{2014} and the duplicate disappears."
                        }
                        {render_target_rail(target.clone(), target_title.clone())}
                        div { class: "mg-search",
                            svg {
                                class: "mg-search-icon",
                                width: "18",
                                height: "18",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "2",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                "aria-hidden": "true",
                                circle { cx: "11", cy: "11", r: "8" }
                                line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
                            }
                            input {
                                id: "bd-merge-search",
                                class: "mg-search-input",
                                "data-testid": "merge-search",
                                "aria-label": "Search your library for a book to merge",
                                r#type: "search",
                                // Merge search scopes to title/authors/series only
                                // (`db::search_books` excludes ISBN), so the
                                // placeholder must not advertise ISBN.
                                placeholder: "Search your library \u{2014} title, author, series\u{2026}",
                                value: "{query}",
                                autofocus: true,
                                oninput: move |evt| {
                                    let q = evt.value();
                                    query.set(q.clone());
                                    run_search(q);
                                },
                            }
                        }
                        if result_count > 0 {
                            div { class: "mg-results-head",
                                span { class: "label", "Results \u{00b7} {result_count}" }
                                span { class: "mono mg-results-hint", "matched on title, author \u{0026} series" }
                            }
                            ul { class: "mg-results",
                                for book in results() {
                                    {render_candidate(book, target_series.clone(), selected)}
                                }
                            }
                        }
                    }
                    if let Some(msg) = error() {
                        p { role: "alert", class: "bd-merge-error", "{msg}" }
                    }
                }
                div { class: "mg-foot",
                    span { class: "mono mg-foot-note",
                        "Nothing is deleted \u{2014} every merge can be undone."
                    }
                    button {
                        class: "btn ghost sm",
                        "data-testid": "merge-cancel",
                        disabled: busy(),
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                }
            }
        }
    }
}

// Debounce sleep, platform-gated like search_palette/overlay.rs: the
// dialog compiles for web (WASM) and server (SSR) targets.
#[cfg(feature = "web")]
async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(all(not(feature = "web"), feature = "server"))]
async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

/// Pinned "this is the keeper" context shown above the search field: the
/// current book's cover + title + author/series, with a "stays" note.
fn render_target_rail(target: EbookMetadata, title: String) -> Element {
    let author = target
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let mut sub = author;
    if let Some(series) = target.series.as_deref() {
        if !sub.is_empty() {
            sub.push_str(" \u{00b7} ");
        }
        sub.push_str(series);
        if let Some(idx) = target.series_index.as_deref() {
            sub.push_str(&format!(" #{idx}"));
        }
    }
    rsx! {
        div { class: "mg-target-rail",
            div { class: "mg-target-cover", Cover { book: target.clone() } }
            div { class: "mg-target-main",
                div { class: "label mg-target-kicker", "Everything folds into" }
                div { class: "mg-target-name",
                    span { class: "mg-target-title", "{title}" }
                    if !sub.is_empty() {
                        span { class: "mono mg-target-sub", "{sub}" }
                    }
                }
            }
            span { class: "mono mg-target-note", "this entry stays" }
        }
    }
}

/// One search hit: cover, title, author · year, an optional "same series"
/// signal, and format badges. Clicking it picks the merge source.
fn render_candidate(
    book: EbookMetadata,
    target_series: Option<String>,
    mut selected: Signal<Option<EbookMetadata>>,
) -> Element {
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    // Primary author only — joining every creator surfaces calibre's
    // book-producer contributor ("calibre (x.y) [url]") as junk, and the
    // keeper rail above already shows just the lead author.
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let year = book
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("");
    let meta = match (author.is_empty(), year.is_empty()) {
        (false, false) => format!("{author} \u{00b7} {year}"),
        (false, true) => author,
        (true, false) => year.to_string(),
        (true, true) => String::new(),
    };
    // A shared series is the strongest "same book, other format" signal —
    // the common ebook↔audiobook merge case.
    let same_series = matches!(
        (target_series.as_deref(), book.series.as_deref()),
        (Some(t), Some(b)) if t.eq_ignore_ascii_case(b)
    );
    let formats = book.formats.clone();
    let cover_book = book.clone();
    let key = book.unique_identifier.clone().unwrap_or_default();
    rsx! {
        li { key: "{key}",
            button {
                class: "mg-row",
                "data-testid": "merge-candidate",
                onclick: move |_| selected.set(Some(book.clone())),
                div { class: "mg-row-cover", Cover { book: cover_book.clone() } }
                div { class: "mg-row-main",
                    div { class: "mg-row-title", "{title}" }
                    if !meta.is_empty() {
                        div { class: "mg-row-meta mono", "{meta}" }
                    }
                }
                div { class: "mg-row-aside",
                    if same_series {
                        span { class: "mg-row-signal",
                            span { class: "mg-row-dot" }
                            "Same series"
                        }
                    }
                    for fmt in formats.iter() {
                        span { class: "mg-fmt", "{fmt}" }
                    }
                }
            }
        }
    }
}

/// Confirm panel once a candidate is picked. (The richer field-by-field
/// reconciliation design is deferred; this keeps the merge actionable.)
fn render_confirm(
    source: EbookMetadata,
    target_title: String,
    busy: bool,
    on_confirm: impl FnMut(()) + 'static,
    on_back: impl FnMut(()) + 'static,
) -> Element {
    let mut on_confirm = on_confirm;
    let mut on_back = on_back;
    let source_title = source
        .title
        .clone()
        .unwrap_or_else(|| source.filename.clone());
    rsx! {
        div { class: "mg-confirm",
            p { class: "mg-confirm-copy",
                strong { "\u{201c}{source_title}\u{201d}" }
                " will be merged into "
                strong { "\u{201c}{target_title}\u{201d}" }
                ". Its files, tags, and reading progress move here; the other entry disappears. This can be undone."
            }
            div { class: "mg-confirm-actions",
                button {
                    class: "btn primary",
                    "data-testid": "merge-confirm",
                    disabled: busy,
                    onclick: move |_| on_confirm(()),
                    if busy { "Merging\u{2026}" } else { "Merge" }
                }
                button {
                    class: "btn ghost",
                    "data-testid": "merge-back",
                    disabled: busy,
                    onclick: move |_| on_back(()),
                    "Back"
                }
            }
        }
    }
}
