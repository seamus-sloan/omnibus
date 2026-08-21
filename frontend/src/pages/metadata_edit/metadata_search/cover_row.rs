//! The cover row of the compare view.
//!
//! A field on this screen like any other, and the only one that cannot stage
//! into the form: `write_override_cover` takes bytes, and the browser cannot
//! fetch a provider's image cross-origin to supply them. So applying a cover
//! means the server fetching the provider URL on the reader's behalf — which
//! makes this the one row that **writes immediately**, and the row has to say
//! so rather than implying it will be saved with the rest.

use dioxus::prelude::*;
use omnibus_shared::{metadata_lookup::ProviderEdition, EbookMetadata};

use super::super::bust_query;
use super::EMPTY;
use crate::components::atrium::Cover;
use crate::contexts::{bump_cover_cache_bust, use_cover_cache_bust};
use crate::{data, media_url, use_server_url};

/// Yours-vs-theirs for the cover, with the same arrow affordance as every
/// other row and a status line that says the change is immediate.
#[component]
pub(super) fn CoverRow(
    uuid: String,
    book: EbookMetadata,
    edition: ProviderEdition,
    source_name: &'static str,
    hydrating: bool,
    on_applied: EventHandler<EbookMetadata>,
) -> Element {
    let server_url = use_server_url();
    let mut busy = use_signal(|| false);
    let mut status: Signal<Option<String>> = use_signal(|| None);
    let global_bust = use_cover_cache_bust().0;

    let source_url = edition
        .cover_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let available = source_url.is_some();

    let on_apply = {
        let uuid = uuid.clone();
        let source_url = source_url.clone();
        move |_| {
            let (Some(url), false) = (source_url.clone(), busy()) else {
                return;
            };
            let uuid = uuid.clone();
            let server_url = server_url.clone();
            busy.set(true);
            status.set(Some("Applying cover\u{2026}".to_string()));
            spawn(async move {
                match data::apply_cover_from_url(&server_url, &uuid, &url).await {
                    Ok(Some(updated)) => {
                        // The cover route caches for a day on an unchanged
                        // URL (`Cache-Control: private, max-age=86400`), so
                        // without this every other view of the book — the
                        // sidebar preview, the grid, the detail page — keeps
                        // serving the old image from the browser's cache.
                        bump_cover_cache_bust(global_bust, &uuid);
                        status.set(Some("Cover updated.".to_string()));
                        on_applied.call(updated);
                    }
                    Ok(None) => status.set(Some("Book not found.".to_string())),
                    Err(e) => status.set(Some(format!("Couldn't apply that cover: {e}"))),
                }
                busy.set(false);
            });
        }
    };

    rsx! {
        div { class: "mes-cover", "data-testid": "mes-row-cover",
            span { class: "mes-field-label", "Cover" }
            span { class: "mes-cover-pair",
                span { class: "mes-cover-cell", "data-testid": "mes-row-cover-current",
                    CoverThumb {
                        book: book.clone(),
                        bust: (global_bust.read().get(&uuid).copied()).unwrap_or(0),
                    }
                }
                button {
                    r#type: "button",
                    class: "mes-apply",
                    "data-testid": "mes-row-cover-apply",
                    aria_label: "Use the cover from {source_name} \u{2014} saves immediately",
                    disabled: !available || hydrating || busy(),
                    onclick: on_apply,
                    "\u{2192}"
                }
                span { class: "mes-cover-cell", "data-testid": "mes-row-cover-source",
                    if let Some(url) = source_url {
                        img { class: "mes-cover-img", src: "{url}", alt: "", loading: "lazy" }
                    } else {
                        span { class: "mes-empty", "{EMPTY}" }
                    }
                }
            }
            // The wording is the contract: this is the one row that doesn't
            // wait for Save.
            span { class: "mono mes-cover-note", role: "status", "data-testid": "mes-row-cover-note",
                if let Some(msg) = status() {
                    "{msg}"
                } else {
                    "The cover applies immediately \u{b7} it isn\u{2019}t staged with the fields"
                }
            }
        }
    }
}

/// The book's current cover, cache-busted off the app-wide registry so an
/// apply from this row (or from the sidebar) is visible without a reload.
///
/// `src_override` stays `None` until the registry has actually moved, so the
/// SSR render and the first WASM paint are byte-identical — the same rule
/// `cover_editor::cover_preview` follows.
#[component]
fn CoverThumb(book: EbookMetadata, bust: u32) -> Element {
    let server_url = use_server_url();
    let src_override = (bust > 0)
        .then(|| {
            book.cover_url
                .as_deref()
                .map(|path| bust_query(&media_url(&server_url, path), bust))
        })
        .flatten();
    rsx! {
        div { class: "mes-cover-thumb",
            Cover { book, src_override }
        }
    }
}
