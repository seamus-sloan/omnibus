//! Tag cloud discovery page — renders all tags scaled by book count, mirroring
//! the `TagCloudScreen` design comp from `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::TagWeight;

use crate::components::{PageError, PageLoading};
use crate::{data, use_server_url, Route};

/// Renders the tag cloud page.
#[component]
pub fn TagCloudPage() -> Element {
    let server_url = use_server_url();
    let mut tags: Signal<Vec<TagWeight>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_tag_cloud(&url).await {
                Ok(t) => {
                    tags.set(t);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }

    let tag_list = tags();
    let total_tags = tag_list.len();
    let max_count = tag_list.iter().map(|t| t.count).max().unwrap_or(1);

    rsx! {
        div { class: "disc-page",
            // Header
            div { class: "disc-tag-header",
                // Mobile-only (CSS-gated via `.screen`) back affordance to
                // search, where this lens is reached from; web keeps its own
                // nav. Same markup on every target so the web SSR/WASM trees
                // stay identical (rule 07).
                Link {
                    to: Route::MobileSearch {},
                    class: "m-icon-btn disc-back",
                    "aria-label": "Back to search",
                    "data-testid": "tags-back",
                    "\u{2190}"
                }
                span { class: "label", "Library lens" }
                h1 { class: "disc-hero-title",
                    "By "
                    em { "tag" }
                }
                p { class: "disc-tag-subtitle",
                    "{total_tags} unique tags \u{b7} click to filter"
                }
            }

            // The cloud
            div { class: "tag-cloud",
                for tag in tag_list.iter() {
                    TagCloudItem { key: "{tag.name}", tag: tag.clone(), max_count }
                }
            }
        }
    }
}

/// Render a single tag in the cloud with size/opacity scaled by weight.
#[component]
fn TagCloudItem(tag: TagWeight, max_count: usize) -> Element {
    // Tag counts and library sizes both comfortably fit a `u32`, well within
    // the f64-exactly-representable integer range. Saturating cast guards
    // against the unlikely-but-possible >4B case rather than asserting.
    let count_f = f64::from(u32::try_from(tag.count).unwrap_or(u32::MAX));
    let max_f = f64::from(u32::try_from(max_count).unwrap_or(u32::MAX));
    let weight = if max_f == 0.0 { 0.0 } else { count_f / max_f };
    let size = 16.0 + (weight * 56.0);
    let opacity = 0.55 + (weight * 0.45);
    let is_high = weight > 0.7;
    let is_italic = weight > 0.5;

    let class = if is_high {
        "tag-cloud-item tag-cloud-item--hi"
    } else {
        "tag-cloud-item"
    };

    let style = format!(
        "font-size: {size:.0}px; opacity: {opacity:.2};{}",
        if is_italic {
            " font-style: italic;"
        } else {
            ""
        }
    );

    let name = tag.name.clone();
    let count = tag.count;
    // Font-size + opacity are the visual weight signal. Screen readers
    // get the same weight info via an explicit aria-label since neither
    // styling property is exposed to assistive tech.
    let aria = format!(
        "{name} — {count} {}",
        if count == 1 { "book" } else { "books" }
    );

    rsx! {
        Link {
            to: Route::Landing {},
            class: "{class}",
            style: "{style}",
            aria_label: "{aria}",
            "{name}"
            span { class: "tag-cloud-count mono", "{count}" }
        }
    }
}
