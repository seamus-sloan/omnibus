//! Stop 02 · Shelf — the book in its company. Series books render the whole
//! series as a cover shelf in reading order with an "Up next" pill; a
//! standalone shows the hand-picked shelves holding it. Both fetch post-mount
//! (rule 07: SSR and the first WASM paint render the same quiet shell).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, SeriesDetail, ShelfSummary};

use crate::components::atrium::Cover;
use crate::{data, use_server_url, Route};

use super::w4::W4ViewFacts;

/// The Shelf stop: series shelf or standalone shelves.
#[component]
pub(super) fn W4ShelfStop(b: EbookMetadata, view: W4ViewFacts) -> Element {
    rsx! {
        if let Some(series_id) = b.series_id {
            W4SeriesShelf {
                series_id,
                series_name: view.series.clone().unwrap_or_default(),
                current_uuid: b.unique_identifier.clone().unwrap_or_default(),
            }
        } else {
            W4StandaloneShelves { uuid: b.unique_identifier.clone().unwrap_or_default() }
        }
    }
}

/// The whole series as covers, in reading order.
#[component]
fn W4SeriesShelf(series_id: i64, series_name: String, current_uuid: String) -> Element {
    let mut detail = use_signal(|| None::<SeriesDetail>);
    use_effect(use_reactive!(|series_id| {
        detail.set(None);
        spawn(async move {
            if let Ok(d) = data::get_series("", series_id).await {
                detail.set(d);
            }
        });
    }));

    let d = detail();
    let count = d.as_ref().map(|d| d.book_count).unwrap_or(0);
    // "Up next": the first unfinished book after the current one in series
    // order. Per-book progress isn't on the listing wire, so the pill marks
    // the next series entry instead.
    let next_uuid: Option<String> = d.as_ref().and_then(|d| {
        let idx = d
            .books
            .iter()
            .position(|x| x.unique_identifier.as_deref() == Some(current_uuid.as_str()))?;
        d.books
            .get(idx + 1)
            .and_then(|x| x.unique_identifier.clone())
    });

    rsx! {
        div { class: "bdw4-k",
            "{series_name}"
            if count > 0 {
                " \u{b7} {count} in your library"
            }
        }
        if let Some(d) = d {
            div { class: "rx-shelf", "data-testid": "bdw4-series-shelf",
                for x in d.books.iter() {
                    {render_series_item(x, &current_uuid, next_uuid.as_deref())}
                }
            }
            div { class: "mono bdw4-quiet-hint",
                Link { to: Route::SeriesDetail { id: series_id }, class: "bdw4-k-link", "series page \u{2192}" }
            }
        } else {
            div { class: "mono bdw4-quiet-hint", "loading the shelf\u{2026}" }
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
fn W4StandaloneShelves(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut shelves = use_signal(|| None::<Vec<ShelfSummary>>);
    {
        let server_url = server_url.clone();
        use_effect(use_reactive!(|uuid| {
            shelves.set(None);
            let url = server_url.clone();
            let uuid = uuid.clone();
            spawn(async move {
                let all = data::list_shelves(&url).await;
                let holding = data::shelves_containing(&url, &uuid).await;
                if let (Ok(all), Ok(ids)) = (all, holding) {
                    let held: Vec<ShelfSummary> =
                        all.into_iter().filter(|s| ids.contains(&s.id)).collect();
                    shelves.set(Some(held));
                }
            });
        }));
    }

    rsx! {
        div { class: "bdw4-k", "Standalone \u{b7} on your shelves" }
        match shelves() {
            Some(held) if !held.is_empty() => rsx! {
                div { class: "bdw4-chips bdw4-shelfchips", "data-testid": "bdw4-shelves",
                    for (i, s) in held.iter().enumerate() {
                        span {
                            key: "{s.id}",
                            class: if i == 0 { "chip bdw4-shelfchip first" } else { "chip bdw4-shelfchip" },
                            style: if let Some(a) = s.accent.clone() { format!("--accent:{a};") } else { String::new() },
                            "{s.name}"
                        }
                    }
                }
                p { class: "mono bdw4-quiet-hint",
                    "open a shelf from the "
                    Link { to: Route::Landing {}, class: "bdw4-k-link", "library page \u{2192}" }
                }
            },
            Some(_) => rsx! {
                div { class: "bdw4-bigquiet", "Not on a shelf yet." }
                p { class: "mono bdw4-quiet-hint",
                    "shelves are made on the "
                    Link { to: Route::Landing {}, class: "bdw4-k-link", "library page \u{2192}" }
                }
            },
            None => rsx! {
                div { class: "mono bdw4-quiet-hint", "checking your shelves\u{2026}" }
            },
        }
    }
}
