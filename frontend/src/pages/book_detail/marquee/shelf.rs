//! Stop 02 · Shelf — the book in its company. Series books render the whole
//! series as a cover shelf in reading order with an "Up next" pill; a
//! standalone shows the hand-picked shelves holding it. Both fetch post-mount
//! (rule 07: SSR and the first WASM paint render the same quiet shell).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, SeriesDetail, ShelfSummary};

use crate::components::atrium::Cover;
use crate::{data, use_server_url, Route};

use super::MarqueeViewFacts;

/// The Shelf stop: series shelf or standalone shelves. The series itself is
/// fetched once by the stage and threaded in, so this stop and the Home
/// kicker read the same record.
#[component]
pub(super) fn MarqueeShelfStop(
    b: EbookMetadata,
    view: MarqueeViewFacts,
    series: Option<SeriesDetail>,
) -> Element {
    rsx! {
        if let Some(series_id) = b.series_id {
            MarqueeSeriesShelf {
                series_id,
                series_name: view.series.clone().unwrap_or_default(),
                current_uuid: b.unique_identifier.clone().unwrap_or_default(),
                detail: series,
            }
        } else {
            MarqueeStandaloneShelves { uuid: b.unique_identifier.clone().unwrap_or_default() }
        }
    }
}

/// The whole series as covers, in reading order.
#[component]
fn MarqueeSeriesShelf(
    series_id: i64,
    series_name: String,
    current_uuid: String,
    detail: Option<SeriesDetail>,
) -> Element {
    let count = detail.as_ref().map(|d| d.book_count).unwrap_or(0);
    // "Up next": the next series entry after this one. Per-book progress
    // isn't on the listing wire, so the pill marks position, not state.
    let next_uuid: Option<String> = detail.as_ref().and_then(|d| {
        let idx = d
            .books
            .iter()
            .position(|x| x.unique_identifier.as_deref() == Some(current_uuid.as_str()))?;
        d.books
            .get(idx + 1)
            .and_then(|x| x.unique_identifier.clone())
    });

    rsx! {
        div { class: "bdmq-k",
            "{series_name}"
            if count > 0 {
                // The design reads "you own N of M", but M (the series' full
                // published length) isn't something the library knows — only
                // what it holds. Name that instead of inventing a total.
                " \u{b7} {count} in your library"
            }
        }
        if let Some(d) = detail {
            div { class: "rx-shelf", "data-testid": "bdmq-series-shelf",
                for x in d.books.iter() {
                    {render_series_item(x, &current_uuid, next_uuid.as_deref())}
                }
            }
            div { class: "mono bdmq-quiet-hint",
                Link { to: Route::SeriesDetail { id: series_id }, class: "bdmq-k-link", "series page \u{2192}" }
            }
        } else {
            div { class: "mono bdmq-quiet-hint", "loading the shelf\u{2026}" }
        }
    }
}

/// One series-shelf cover with its caption and (maybe) the Up-next pill.
fn render_series_item(x: &EbookMetadata, current_uuid: &str, next_uuid: Option<&str>) -> Element {
    let x_uuid = x.unique_identifier.clone().unwrap_or_default();
    let current = x_uuid == current_uuid;
    let title = x.display_title();
    let sub = match (x.series_index.as_deref(), current) {
        (Some(n), true) => format!("Book {n} \u{b7} this book"),
        (Some(n), false) => format!("Book {n}"),
        (None, true) => "this book".to_string(),
        (None, false) => String::new(),
    };
    rsx! {
        Link {
            key: "{x_uuid}",
            to: Route::BookDetail { uuid: x_uuid.clone() },
            class: if current { "cover-link rx-shelf-item current" } else { "cover-link rx-shelf-item" },
            if next_uuid == Some(x_uuid.as_str()) {
                span { class: "rx-upnext", "Up next" }
            }
            Cover { book: x.clone() }
            div { class: "rx-shelf-cap",
                div { class: "t", "{title}" }
                if !sub.is_empty() {
                    div { class: "u", "{sub}" }
                }
            }
        }
    }
}

/// Standalone: the hand-picked shelves holding this book, as chips.
#[component]
fn MarqueeStandaloneShelves(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut shelves = use_signal(|| None::<Vec<ShelfSummary>>);
    // A fast SPA hop between books can leave the previous book's shelf fetch
    // in flight; drop its result rather than showing it under the new book.
    let mut load_seq = use_signal(|| 0u64);
    {
        let server_url = server_url.clone();
        use_effect(use_reactive!(|uuid| {
            let my_load = *load_seq.peek() + 1;
            load_seq.set(my_load);
            shelves.set(None);
            let url = server_url.clone();
            let uuid = uuid.clone();
            spawn(async move {
                let all = data::list_shelves(&url).await;
                let holding = data::shelves_containing(&url, &uuid).await;
                if let (Ok(all), Ok(ids)) = (all, holding) {
                    if *load_seq.peek() == my_load {
                        let held: Vec<ShelfSummary> =
                            all.into_iter().filter(|s| ids.contains(&s.id)).collect();
                        shelves.set(Some(held));
                    }
                }
            });
        }));
    }

    rsx! {
        div { class: "bdmq-k", "Standalone \u{b7} on your shelves" }
        match shelves() {
            Some(held) if !held.is_empty() => rsx! {
                div { class: "bdmq-chips bdmq-shelfchips", "data-testid": "bdmq-shelves",
                    for (i, s) in held.iter().enumerate() {
                        span {
                            key: "{s.id}",
                            class: if i == 0 { "chip bdmq-shelfchip first" } else { "chip bdmq-shelfchip" },
                            style: if let Some(a) = s.accent.clone() { format!("--accent:{a};") } else { String::new() },
                            "{s.name}"
                        }
                    }
                }
                p { class: "mono bdmq-quiet-hint",
                    "open a shelf from the "
                    Link { to: Route::Landing {}, class: "bdmq-k-link", "library page \u{2192}" }
                }
            },
            Some(_) => rsx! {
                div { class: "bdmq-bigquiet", "Not on a shelf yet." }
                p { class: "mono bdmq-quiet-hint",
                    "shelves are made on the "
                    Link { to: Route::Landing {}, class: "bdmq-k-link", "library page \u{2192}" }
                }
            },
            None => rsx! {
                div { class: "mono bdmq-quiet-hint", "checking your shelves\u{2026}" }
            },
        }
    }
}
