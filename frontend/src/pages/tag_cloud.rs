//! Tag cloud discovery page — renders all tags scaled by book count, mirroring
//! the `TagCloudScreen` design comp from `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::TagWeight;

use crate::{data, use_server_url, Route};

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
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    if loading() {
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }

    let tag_list = tags();
    let total_tags = tag_list.len();
    let max_count = tag_list.iter().map(|t| t.count).max().unwrap_or(1);

    rsx! {
        div { class: "disc-page",
            // Header
            div { class: "disc-tag-header",
                span { class: "label", "Library lens" }
                h1 { class: "disc-hero-title",
                    "By "
                    em { "tag" }
                }
                p { class: "disc-tag-subtitle",
                    "{total_tags} unique tags \u{b7} click to filter"
                }
            }

            // Two-column: cloud + sidebar stub
            div { class: "disc-two-col",
                // The cloud
                div { class: "tag-cloud",
                    for tag in tag_list.iter() {
                        { render_tag_item(tag, max_count) }
                    }
                }
                // Overlap sidebar — placeholder for future tag analysis
                aside { class: "card", aria_hidden: "true",
                    // TODO(F1.13): Tag overlap matrix — show related tags when
                    // one is selected. Stubbed out until the backend supports
                    // co-occurrence queries.
                }
            }
        }
    }
}

/// Render a single tag in the cloud with size/opacity scaled by weight.
fn render_tag_item(tag: &TagWeight, max_count: usize) -> Element {
    let weight = tag.count as f64 / max_count as f64;
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

    rsx! {
        Link {
            to: Route::Landing {},
            class: "{class}",
            style: "{style}",
            "{name}"
            span { class: "tag-cloud-count mono", "{count}" }
        }
    }
}
