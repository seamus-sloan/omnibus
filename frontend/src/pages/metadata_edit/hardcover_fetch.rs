//! "Fetch from Hardcover" panel: an on-demand lookup that previews
//! Hardcover's canonical title/author/description/series/ISBN-13 next to
//! the current form values, with per-field and select-all controls. Applying
//! only stages the chosen values into the form's existing signals — the
//! page's already-tested `SaveBar`/`on_save` path is what actually writes
//! them through as metadata overrides, so this module never talks to the
//! overrides endpoint itself.
//!
//! Hidden entirely when no Hardcover key is configured
//! (`data::hardcover_fetch_available`), mirroring the suggestions strip's
//! disablement rule. Cover art is out of scope: applying an overridden
//! cover (#983) hasn't landed in this codebase yet.

use dioxus::prelude::*;
use omnibus_shared::metadata_fetch::{FetchedBookMetadata, HardcoverFetchResult};

use super::form_grid::FormFields;
use crate::{data, use_server_url};

/// What the fetch action is currently doing.
#[derive(Clone, PartialEq)]
enum FetchState {
    Idle,
    NotConfigured,
    NotFound,
    Error(String),
    Found(FetchedBookMetadata),
}

/// Top-level panel: the fetch button plus whatever state it produced.
/// Renders nothing (SSR and first paint agree) until the availability check
/// resolves post-mount.
#[component]
pub(super) fn HardcoverFetchPanel(uuid: String, fields: FormFields) -> Element {
    let server_url = use_server_url();
    let mut available = use_signal(|| false);
    let mut state = use_signal(|| FetchState::Idle);
    let mut busy = use_signal(|| false);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            if let Ok(true) = data::hardcover_fetch_available(&url).await {
                available.set(true);
            }
        });
    });

    if !available() {
        return rsx! {};
    }

    let on_fetch = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let url = server_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            let outcome = data::fetch_hardcover_metadata(&url, &uuid).await;
            state.set(match outcome {
                Ok(HardcoverFetchResult::NotConfigured) => FetchState::NotConfigured,
                Ok(HardcoverFetchResult::NotFound) => FetchState::NotFound,
                Ok(HardcoverFetchResult::Found(meta)) => FetchState::Found(meta),
                Err(e) => FetchState::Error(e.to_string()),
            });
            busy.set(false);
        });
    };

    rsx! {
        div { class: "hcf-panel", "data-testid": "hardcover-fetch",
            button {
                r#type: "button",
                class: "btn hcf-fetch-btn",
                "data-testid": "hardcover-fetch-btn",
                disabled: busy(),
                onclick: on_fetch,
                if busy() { "Fetching\u{2026}" } else { "Fetch from Hardcover" }
            }
            {match state() {
                FetchState::NotConfigured => rsx! {
                    div { class: "hcf-note", role: "status", "data-testid": "hardcover-fetch-not-configured",
                        "Hardcover isn't configured for this server."
                    }
                },
                FetchState::NotFound => rsx! {
                    div { class: "hcf-note", role: "status", "data-testid": "hardcover-fetch-not-found",
                        "No match found on Hardcover for this book."
                    }
                },
                FetchState::Error(msg) => rsx! {
                    div { class: "hcf-error", role: "alert", "data-testid": "hardcover-fetch-error", "{msg}" }
                },
                FetchState::Found(meta) => rsx! {
                    HardcoverPreview { fields, meta }
                },
                FetchState::Idle => rsx! {},
            }}
        }
    }
}

/// One selectable diff row's checkbox signal, keyed by field so
/// `HardcoverPreview` can build/apply them without a dozen named locals.
#[derive(Clone, Copy)]
struct PreviewSelections {
    title: Signal<bool>,
    authors: Signal<bool>,
    description: Signal<bool>,
    series: Signal<bool>,
    series_index: Signal<bool>,
    isbn13: Signal<bool>,
}

/// The preview/apply body once a match was found: one diff row per
/// non-empty fetched field, a "Select all" shortcut, and "Apply selected"
/// (stages into the form signals — does not save).
#[component]
fn HardcoverPreview(fields: FormFields, meta: FetchedBookMetadata) -> Element {
    let sel = PreviewSelections {
        title: use_signal(|| false),
        authors: use_signal(|| false),
        description: use_signal(|| false),
        series: use_signal(|| false),
        series_index: use_signal(|| false),
        isbn13: use_signal(|| false),
    };

    let has_title = non_empty(meta.title.as_deref());
    let has_authors = !meta.authors.is_empty();
    let has_description = non_empty(meta.description.as_deref());
    let has_series = non_empty(meta.series.as_deref());
    let has_series_index = non_empty(meta.series_index.as_deref());
    let has_isbn13 = non_empty(meta.isbn13.as_deref());

    if !has_title
        && !has_authors
        && !has_description
        && !has_series
        && !has_series_index
        && !has_isbn13
    {
        return rsx! {
            div { class: "hcf-note", role: "status", "data-testid": "hardcover-fetch-empty",
                "Hardcover had no usable fields for this book."
            }
        };
    }

    let select_all = move |_| {
        let mut sel = sel;
        sel.title.set(has_title);
        sel.authors.set(has_authors);
        sel.description.set(has_description);
        sel.series.set(has_series);
        sel.series_index.set(has_series_index);
        sel.isbn13.set(has_isbn13);
    };
    let on_apply = apply_handler(fields, sel, meta.clone());

    rsx! {
        div { class: "hcf-preview", "data-testid": "hardcover-fetch-preview",
            if has_title {
                HcfRow {
                    label: "Title",
                    current: fields.title.read().clone(),
                    fetched: meta.title.clone().unwrap_or_default(),
                    selected: sel.title,
                    testid: "hardcover-fetch-title",
                }
            }
            if has_authors {
                HcfRow {
                    label: "Author(s)",
                    current: fields.authors.read().join(", "),
                    fetched: meta.authors.join(", "),
                    selected: sel.authors,
                    testid: "hardcover-fetch-authors",
                }
            }
            if has_description {
                HcfRow {
                    label: "Description",
                    current: fields.description.read().clone(),
                    fetched: meta.description.clone().unwrap_or_default(),
                    selected: sel.description,
                    testid: "hardcover-fetch-description",
                }
            }
            if has_series {
                HcfRow {
                    label: "Series",
                    current: fields.series.read().clone(),
                    fetched: meta.series.clone().unwrap_or_default(),
                    selected: sel.series,
                    testid: "hardcover-fetch-series",
                }
            }
            if has_series_index {
                HcfRow {
                    label: "Book #",
                    current: fields.series_index.read().clone(),
                    fetched: meta.series_index.clone().unwrap_or_default(),
                    selected: sel.series_index,
                    testid: "hardcover-fetch-series-index",
                }
            }
            if has_isbn13 {
                HcfRow {
                    label: "ISBN-13",
                    current: fields.isbn13.read().clone(),
                    fetched: meta.isbn13.clone().unwrap_or_default(),
                    selected: sel.isbn13,
                    testid: "hardcover-fetch-isbn13",
                }
            }
            div { class: "hcf-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "hardcover-fetch-select-all",
                    onclick: select_all,
                    "Select all"
                }
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "hardcover-fetch-apply",
                    onclick: on_apply,
                    "Apply selected"
                }
            }
        }
    }
}

/// `true` for a value that carries visible content once trimmed.
fn non_empty(v: Option<&str>) -> bool {
    v.is_some_and(|s| !s.trim().is_empty())
}

/// Build the "Apply selected" click handler: for each checked field, copy
/// the fetched value into the matching form signal. Unchecked fields, and
/// fields Hardcover didn't return, are left untouched.
fn apply_handler(
    fields: FormFields,
    sel: PreviewSelections,
    meta: FetchedBookMetadata,
) -> impl FnMut(Event<MouseData>) + 'static {
    let mut title = fields.title;
    let mut authors = fields.authors;
    let mut description = fields.description;
    let mut series = fields.series;
    let mut series_index = fields.series_index;
    let mut isbn13 = fields.isbn13;
    move |_| {
        if (sel.title)() {
            if let Some(v) = meta.title.clone() {
                title.set(v);
            }
        }
        if (sel.authors)() && !meta.authors.is_empty() {
            authors.set(meta.authors.clone());
        }
        if (sel.description)() {
            if let Some(v) = meta.description.clone() {
                description.set(v);
            }
        }
        if (sel.series)() {
            if let Some(v) = meta.series.clone() {
                series.set(v);
            }
        }
        if (sel.series_index)() {
            if let Some(v) = meta.series_index.clone() {
                series_index.set(v);
            }
        }
        if (sel.isbn13)() {
            if let Some(v) = meta.isbn13.clone() {
                isbn13.set(v);
            }
        }
    }
}

/// One current-vs-fetched diff row with its apply checkbox. The checkbox
/// carries its own `aria-label` ("Apply fetched {label}") rather than
/// inheriting the wrapping `<label>`'s visible "{label}" text as its
/// accessible name — that text is identical to the real form field's own
/// label (e.g. both say "Title"), which made `getByLabel("Title")` resolve
/// to two elements and, for a screen-reader user, made the two controls
/// indistinguishable by name. `aria-label` takes precedence over the
/// implicit label association, so the visible row heading is unchanged.
#[component]
fn HcfRow(
    label: &'static str,
    current: String,
    fetched: String,
    selected: Signal<bool>,
    testid: String,
) -> Element {
    let mut selected = selected;
    let checkbox_label = format!("Apply fetched {label}");
    rsx! {
        label { class: "hcf-row", "data-testid": "{testid}",
            input {
                r#type: "checkbox",
                "data-testid": "{testid}-checkbox",
                aria_label: "{checkbox_label}",
                checked: selected(),
                onchange: move |e| selected.set(e.checked()),
            }
            div { class: "hcf-row-body",
                div { class: "hcf-row-label", "{label}" }
                div { class: "hcf-row-values",
                    span { class: "hcf-row-current", "data-testid": "{testid}-current", "{current}" }
                    span { class: "hcf-row-arrow", aria_hidden: "true", "\u{2192}" }
                    span { class: "hcf-row-fetched", "data-testid": "{testid}-fetched", "{fetched}" }
                }
            }
        }
    }
}

// Regression coverage for the hook-order invariant `HardcoverFetchPanel`
// must hold across its `available: false -> true` transition (rule
// `07-hydration.md`'s "declare every hook unconditionally, in the same
// order" — here triggered by a runtime signal flip rather than a
// `#[cfg(feature = "web")]` gate). The real panel resolves `available`
// through a live server function, which needs a running Axum request
// context this unit test doesn't have, so the harness below reproduces the
// exact hook shape the fixed component uses — a gate signal read first, two
// more `use_signal`s declared unconditionally, then the early-return gate —
// and drives it through a real `VirtualDom` across the transition. A live
// `ui-validate` pass against the real panel is still recommended before
// merge.
#[cfg(all(test, feature = "server"))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Carries the harness's gate signal handle out to the test once the
    /// root component has mounted, so the test can flip it from outside the
    /// component tree (mirroring the real panel's async effect resolving
    /// after first paint).
    #[derive(Clone)]
    struct Capture(Rc<RefCell<Option<Signal<bool>>>>);

    impl PartialEq for Capture {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    fn gate_root(capture: Capture) -> Element {
        let available = use_signal(|| false);
        use_hook(|| {
            *capture.0.borrow_mut() = Some(available);
        });
        rsx! {
            GateHarness { available }
        }
    }

    /// Mirrors `HardcoverFetchPanel`'s fixed shape: the gate signal, then
    /// two more `use_signal`s declared unconditionally, then the
    /// early-return gate. Before the fix, `state`/`busy` were declared
    /// *after* the gate, so a render while ungated called one fewer hook
    /// than a render post-transition.
    #[component]
    fn GateHarness(available: Signal<bool>) -> Element {
        let state = use_signal(|| 0u32);
        let busy = use_signal(|| false);

        if !available() {
            return rsx! {
                div { "data-testid": "gated" }
            };
        }

        rsx! {
            div { "data-testid": "revealed", "{state()}-{busy()}" }
        }
    }

    /// The core regression: flipping the gate signal after mount must not
    /// panic (Dioxus panics on a hook-count mismatch between renders of one
    /// scope) and must reveal the post-gate markup once it does.
    #[test]
    fn gate_harness_survives_the_available_transition_without_a_hook_order_panic() {
        let captured = Capture(Rc::new(RefCell::new(None)));
        let mut dom = VirtualDom::new_with_props(gate_root, captured.clone());
        dom.rebuild_in_place();

        let before = dioxus::ssr::render(&dom);
        assert!(
            before.contains("gated"),
            "gated before availability resolves: {before}"
        );
        assert!(!before.contains("revealed"));

        let mut available = captured
            .0
            .borrow_mut()
            .take()
            .expect("gate_root captures its signal on first mount");
        available.set(true);

        // Applying the pending rerender is the operation that would panic
        // on a hook-count mismatch; it must not, now that `state`/`busy`
        // are declared unconditionally ahead of the gate.
        dom.render_immediate_to_vec();

        let after = dioxus::ssr::render(&dom);
        assert!(
            after.contains("revealed"),
            "revealed after the transition: {after}"
        );
        assert!(!after.contains("gated"));
    }
}
