//! The edition picker: a two-screen overlay that finds this book at every
//! configured provider and moves the fields you pick into the edit form.
//!
//! Shaped after the check-in flow — a [`Stage`] machine rendered in a centred
//! card over a scrim — because it answers the same question that flow does
//! ("which of these is your book?") and should not feel like a different app
//! for asking it.
//!
//! Rule 07 shapes the top of this module: the trigger's availability check
//! and every fetch live in effects and handlers, never in a `cfg` around the
//! rsx, and a closed overlay is an absent node — so SSR and the first WASM
//! paint agree, and the reveal happens once, after hydration.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::{ProviderEdition, ProviderSearchSource};
use omnibus_shared::EbookMetadata;

use super::form_grid::FormFields;
use crate::components::glyphs::sparkle_glyph;
use crate::{data, use_server_url};

mod candidates;
mod compare;
mod cover_row;
mod field;
mod results;
mod sources;

use compare::CompareScreen;
use results::ResultsScreen;

/// Which screen the overlay is showing.
///
/// Two, plus their in-between states. More than that and the reader is
/// filling in a wizard rather than picking a book.
#[derive(Clone, PartialEq)]
pub(super) enum Stage {
    /// A search is in flight. The opening state — the query is already known,
    /// so the overlay searches on open rather than asking first.
    Searching,
    /// Candidates are on screen.
    Results,
    /// The search itself failed. Distinct from a search that ran and found
    /// nothing, which is `Results` with an empty list and a per-source line
    /// saying why.
    Failed(String),
    /// One candidate, beside the book. Boxed because it dwarfs every other
    /// variant, and `Stage` is cloned on each read of the signal.
    Compare(Box<ProviderEdition>),
}

/// Everything the overlay's screens read and its handlers write.
#[derive(Clone, Copy)]
pub(super) struct PickerState {
    pub(super) stage: Signal<Stage>,
    pub(super) query: Signal<String>,
    pub(super) editions: Signal<Vec<ProviderEdition>>,
    pub(super) sources: Signal<Vec<ProviderSearchSource>>,
    /// A selected candidate is re-fetched in full behind the reveal; until it
    /// lands the compare screen is showing the thinner search hit, so nothing
    /// on it may be applied.
    pub(super) hydrating: Signal<bool>,
}

impl PartialEq for PickerState {
    fn eq(&self, other: &Self) -> bool {
        self.stage == other.stage
    }
}

/// Slugify a field label for a `data-testid`, matching the edit form's own
/// label slugging without its `me-` prefix.
fn field_slug(label: &str) -> String {
    label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// The search a freshly-opened overlay runs: the book's own title and primary
/// author, which is the query the reader wanted in the overwhelmingly common
/// case.
fn seed_query(fields: FormFields) -> String {
    let title = fields.title.peek().clone();
    let author = fields.authors.peek().first().cloned();
    compose_query(&title, author.as_deref())
}

/// The seeded query itself, split from the signal reads so the rule it
/// encodes — never a leading or trailing space, never whitespace alone — is
/// testable without a Dioxus runtime.
fn compose_query(title: &str, author: Option<&str>) -> String {
    let title = title.trim();
    let author = author.map(str::trim).unwrap_or_default();
    match (title.is_empty(), author.is_empty()) {
        (true, _) => author.to_string(),
        (false, true) => title.to_string(),
        (false, false) => format!("{title} {author}"),
    }
}

/// The trigger that opens the picker, and the overlay itself while open.
///
/// Renders nothing at all until the provider catalog says at least one source
/// is usable — offering a search that could only fail is worse than not
/// offering one.
#[component]
pub(super) fn MetadataSearchPanel(
    fields: FormFields,
    /// The book as loaded — the baseline the save bar counts against, and so
    /// the only honest answer to "is this field carrying a change?".
    orig: Signal<EbookMetadata>,
    uuid: String,
    book: EbookMetadata,
    on_cover_applied: EventHandler<EbookMetadata>,
) -> Element {
    let server_url = use_server_url();
    let mut available = use_signal(|| false);
    // Declared ahead of the availability gate, unconditionally: a hook count
    // that changes across the reveal panics Dioxus (rule 07).
    let mut open = use_signal(|| false);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            if let Ok(providers) = data::list_metadata_providers(&url).await {
                available.set(providers.iter().any(|p| p.configured));
            }
        });
    });

    if !available() {
        return rsx! {};
    }

    rsx! {
        button {
            r#type: "button",
            class: "btn mes-open-btn",
            "data-testid": "metadata-search-btn",
            onclick: move |_| open.set(true),
            span { class: "mes-open-mark", aria_hidden: "true", {sparkle_glyph(16)} }
            "Fetch metadata"
        }
        // Mounted only while open, so every fresh open starts a fresh search
        // rather than showing the last one's results.
        if open() {
            SearchOverlay {
                fields,
                orig,
                uuid,
                book,
                on_cover_applied,
                on_close: move |()| open.set(false),
            }
        }
    }
}

/// The scrim + card wrapper. Owns the flow's state, so closing and reopening
/// resets it.
#[component]
fn SearchOverlay(
    fields: FormFields,
    orig: Signal<EbookMetadata>,
    uuid: String,
    book: EbookMetadata,
    on_cover_applied: EventHandler<EbookMetadata>,
    on_close: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let state = PickerState {
        stage: use_signal(|| Stage::Searching),
        query: use_signal(|| seed_query(fields)),
        editions: use_signal(Vec::new),
        sources: use_signal(Vec::new),
        hydrating: use_signal(|| false),
    };
    let run_search = search_handler(state, server_url.clone());
    let on_select = select_handler(state, server_url);

    // The query is already known, so the overlay opens searching rather than
    // asking the reader to press a button to ask the question they just asked.
    use_hook(|| run_search.call(()));

    rsx! {
        div {
            class: "mes-scrim",
            "data-testid": "metadata-search-scrim",
            onclick: move |_| on_close.call(()),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div {
                class: "mes-panel",
                role: "dialog",
                aria_modal: "true",
                aria_label: "Find this edition",
                "data-testid": "metadata-search",
                onclick: move |e| e.stop_propagation(),
                button {
                    r#type: "button",
                    class: "mes-close",
                    "data-testid": "mes-close",
                    aria_label: "Close",
                    onclick: move |_| on_close.call(()),
                    "\u{d7}"
                }
                {match (state.stage)() {
                    Stage::Compare(edition) => rsx! {
                        CompareScreen {
                            edition: *edition,
                            fields,
                            orig,
                            uuid,
                            book,
                            hydrating: (state.hydrating)(),
                            on_back: move |()| state.stage.clone().set(Stage::Results),
                            on_cover_applied,
                            on_done: move |()| on_close.call(()),
                        }
                    },
                    stage => rsx! {
                        ResultsScreen { state, stage, on_search: run_search, on_select }
                    },
                }}
            }
        }
    }
}

/// Run the fan-out search for the current query.
fn search_handler(state: PickerState, server_url: String) -> EventHandler<()> {
    let mut stage = state.stage;
    let mut editions = state.editions;
    let mut sources = state.sources;
    let query = state.query;
    EventHandler::new(move |()| {
        let q = query.peek().trim().to_string();
        if q.is_empty() {
            stage.set(Stage::Results);
            return;
        }
        let url = server_url.clone();
        stage.set(Stage::Searching);
        spawn(async move {
            match data::search_editions(&url, &q).await {
                Ok(found) => {
                    editions.set(candidates::in_stable_order(found.editions, &q));
                    sources.set(found.sources);
                    stage.set(Stage::Results);
                }
                Err(e) => stage.set(Stage::Failed(e.to_string())),
            }
        });
    })
}

/// Open the compare screen for one candidate, and re-fetch it in full behind
/// the reveal.
///
/// The merge is one-directional on purpose: the detail record fills in what
/// the list row lacked and can never take a field away from it, so the row
/// the reader clicked is still the row they get.
fn select_handler(state: PickerState, server_url: String) -> EventHandler<ProviderEdition> {
    let mut stage = state.stage;
    let mut hydrating = state.hydrating;
    EventHandler::new(move |edition: ProviderEdition| {
        stage.set(Stage::Compare(Box::new(edition.clone())));
        hydrating.set(true);
        let url = server_url.clone();
        spawn(async move {
            let fetched =
                data::hydrate_edition(&url, edition.source, &edition.provider_ref, &edition.isbn13)
                    .await;
            // A slow hydrate must not overwrite a candidate the reader has
            // since replaced, or reappear after they went back to the list.
            let showing_ours = matches!(
                &*stage.peek(),
                Stage::Compare(shown)
                    if shown.source == edition.source
                        && shown.provider_ref == edition.provider_ref
            );
            if showing_ours {
                if let Ok(Some(mut full)) = fetched {
                    full.fill_missing_from(&edition);
                    stage.set(Stage::Compare(Box::new(full)));
                }
            }
            // Left alone only while a *newer* selection is in flight — that
            // request owns the flag and will clear it when it lands.
            if showing_ours || !matches!(&*stage.peek(), Stage::Compare(_)) {
                hydrating.set(false);
            }
        });
    })
}

#[cfg(test)]
mod tests;
