//! Create-shelf modal — the two-body form for smart and hand-picked shelves.
//!
//! Smart: a `match [any|all]` + condition-row editor with a live preview pane.
//! Hand-picked: a searchable cover grid whose selection becomes the shelf's
//! initial book list. Both bodies share the header (name + kind + visibility)
//! and submit through [`crate::data::create_shelf`].

use dioxus::prelude::*;
use omnibus_shared::{
    CreateShelfRequest, EbookMetadata, MatchMode, Shelf, ShelfKind, ShelfRule, Visibility,
};

use crate::components::library_picker_grid::{filter_library, use_library_fetch};
use crate::components::shelf_rule_builder::{RuleBuilder, RuleDraft};
use crate::components::LibraryPickerGrid;
use crate::{data, use_server_url};

#[cfg(test)]
mod tests;

/// Create-shelf modal. Emits the created shelf via `on_created`.
#[component]
pub fn CreateShelfModal(on_close: EventHandler<()>, on_created: EventHandler<Shelf>) -> Element {
    let server_url = use_server_url();
    let name = use_signal(String::new);
    let kind = use_signal(|| ShelfKind::Smart);
    let visibility = use_signal(|| Visibility::Private);
    let error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);

    // Smart-body state.
    let mut match_mode = use_signal(|| MatchMode::All);
    let rules = use_signal(|| vec![RuleDraft::default()]);

    // Hand-picked state.
    let picked = use_signal(Vec::<String>::new);

    let on_submit = build_on_submit(
        server_url.clone(),
        CreateShelfFormSignals {
            kind,
            name,
            visibility,
            match_mode,
            rules,
            picked,
            error,
            saving,
        },
        on_created,
    );

    let picked_count = picked.read().len();
    let create_label = create_label(kind(), picked_count);

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "create-shelf-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),

                {create_shelf_head(name, kind, visibility)}

                div { class: "shelf-modal-body",
                    match kind() {
                        ShelfKind::Smart => rsx! {
                            RuleBuilder {
                                match_mode: match_mode(),
                                rules,
                                on_match_mode: move |m| match_mode.set(m),
                                server_url: server_url.clone(),
                            }
                        },
                        ShelfKind::Manual | ShelfKind::Wishlist => rsx! {
                            PickerBody { picked, server_url: server_url.clone() }
                        },
                    }
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "shelf-create-error",
                        "{msg}"
                    }
                }

                {create_shelf_foot(saving(), create_label, on_close, on_submit)}
            }
        }
    }
}

/// Form-state signals [`build_on_submit`] reads/writes. `Copy` (Dioxus
/// signals), so grouping them keeps the function under clippy's
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
struct CreateShelfFormSignals {
    kind: Signal<ShelfKind>,
    name: Signal<String>,
    visibility: Signal<Visibility>,
    match_mode: Signal<MatchMode>,
    rules: Signal<Vec<RuleDraft>>,
    picked: Signal<Vec<String>>,
    error: Signal<Option<String>>,
    saving: Signal<bool>,
}

/// Builds the create-shelf submit handler: encodes the current form state
/// via [`build_create_request`], then saves it and reports the result back
/// through `error`/`saving`/`on_created`.
fn build_on_submit(
    server_url: String,
    sig: CreateShelfFormSignals,
    on_created: EventHandler<Shelf>,
) -> EventHandler<MouseEvent> {
    let CreateShelfFormSignals {
        kind,
        name,
        visibility,
        match_mode,
        rules,
        picked,
        mut error,
        mut saving,
    } = sig;
    EventHandler::new(move |_| {
        if saving() {
            return;
        }
        let req = build_create_request(
            kind(),
            name(),
            visibility(),
            match_mode(),
            &rules.read(),
            &picked.read(),
        );
        let url = server_url.clone();
        saving.set(true);
        error.set(None);
        spawn(async move {
            match data::create_shelf(&url, req).await {
                Ok(shelf) => on_created.call(shelf),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    })
}

/// Modal head: name input, smart/hand-picked kind toggle, and the shared
/// visibility toggle.
fn create_shelf_head(
    mut name: Signal<String>,
    mut kind: Signal<ShelfKind>,
    mut visibility: Signal<Visibility>,
) -> Element {
    rsx! {
        div { class: "shelf-modal-head",
            input {
                r#type: "text",
                class: "shelf-name-input",
                placeholder: "Shelf name\u{2026}",
                "data-testid": "shelf-name-input",
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            div { class: "shelf-kind-toggle",
                button {
                    r#type: "button",
                    class: "shelf-toggle-btn",
                    "aria-pressed": if kind() == ShelfKind::Smart { "true" } else { "false" },
                    "data-testid": "shelf-kind-smart",
                    onclick: move |_| kind.set(ShelfKind::Smart),
                    "Smart"
                }
                button {
                    r#type: "button",
                    class: "shelf-toggle-btn",
                    "aria-pressed": if kind() == ShelfKind::Manual { "true" } else { "false" },
                    "data-testid": "shelf-kind-manual",
                    onclick: move |_| kind.set(ShelfKind::Manual),
                    "Hand-picked"
                }
            }
            VisibilityToggle { visibility: visibility(), on_change: move |v| visibility.set(v) }
        }
    }
}

/// Modal foot: cancel + submit buttons; submit disables and swaps its label
/// while a save request is in flight.
fn create_shelf_foot(
    saving: bool,
    create_label: String,
    on_close: EventHandler<()>,
    on_submit: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "shelf-modal-foot",
            button {
                r#type: "button",
                class: "btn shelf-btn-ghost",
                onclick: move |_| on_close.call(()),
                "Cancel"
            }
            button {
                r#type: "button",
                class: "btn shelf-btn-primary",
                "data-testid": "shelf-create-submit",
                disabled: saving,
                onclick: move |e| on_submit.call(e),
                if saving { "Creating\u{2026}" } else { "{create_label}" }
            }
        }
    }
}

/// Build the create-shelf request for the current form state. Smart shelves
/// carry the encoded rule set and no book list; Manual — and Wishlist, which
/// the kind toggle can never reach since it's system-provisioned — carry the
/// picked book list and no rules.
fn build_create_request(
    kind: ShelfKind,
    name: String,
    visibility: Visibility,
    match_mode: MatchMode,
    rules: &[RuleDraft],
    picked: &[String],
) -> CreateShelfRequest {
    match kind {
        ShelfKind::Smart => {
            let wire: Vec<ShelfRule> = rules.iter().filter_map(RuleDraft::to_rule).collect();
            CreateShelfRequest {
                kind: ShelfKind::Smart,
                name,
                description: None,
                visibility,
                match_mode: Some(match_mode),
                rules: wire,
                book_uuids: Vec::new(),
            }
        }
        ShelfKind::Manual | ShelfKind::Wishlist => CreateShelfRequest {
            kind: ShelfKind::Manual,
            name,
            description: None,
            visibility,
            match_mode: None,
            rules: Vec::new(),
            book_uuids: picked.to_vec(),
        },
    }
}

/// Submit-button label: plain for a smart shelf, a running picked-count for a
/// hand-picked one.
fn create_label(kind: ShelfKind, picked_count: usize) -> String {
    match kind {
        ShelfKind::Smart => "Create".to_string(),
        // Wishlist can't be reached by the toggle; grouped with Manual.
        ShelfKind::Manual | ShelfKind::Wishlist => format!("Create \u{b7} {picked_count}"),
    }
}

/// Private/Public segmented control shared by the create- and edit-shelf
/// modal headers.
#[component]
pub fn VisibilityToggle(visibility: Visibility, on_change: EventHandler<Visibility>) -> Element {
    rsx! {
        div { class: "shelf-vis-toggle",
            button {
                r#type: "button",
                class: "shelf-toggle-btn",
                "aria-pressed": if visibility == Visibility::Private { "true" } else { "false" },
                "data-testid": "shelf-vis-private",
                onclick: move |_| on_change.call(Visibility::Private),
                "Private"
            }
            button {
                r#type: "button",
                class: "shelf-toggle-btn",
                "aria-pressed": if visibility == Visibility::Public { "true" } else { "false" },
                "data-testid": "shelf-vis-public",
                onclick: move |_| on_change.call(Visibility::Public),
                "Public"
            }
        }
    }
}

/// Hand-picked picker: a searchable, selectable cover grid over the library.
#[component]
fn PickerBody(picked: Signal<Vec<String>>, server_url: String) -> Element {
    let library = use_signal(Vec::<EbookMetadata>::new);
    let mut query = use_signal(String::new);

    use_library_fetch(server_url.clone(), library);

    // Memoized so filter only reruns when library/query change, not on every render.
    let filtered = use_memo(move || {
        let library_books = library.read();
        filter_library(&library_books, &query.read())
            .into_iter()
            .cloned()
            .collect::<Vec<EbookMetadata>>()
    });
    let filtered = filtered();
    let picked_count = picked.read().len();

    rsx! {
        div { class: "shelf-picker",
            div { class: "shelf-picker-bar",
                input {
                    r#type: "search",
                    class: "shelf-picker-search",
                    placeholder: "Search your library\u{2026}",
                    "data-testid": "shelf-picker-search",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
                span { class: "mono shelf-picker-count", "On this shelf \u{b7} {picked_count}" }
            }
            LibraryPickerGrid { books: filtered, server_url: server_url.clone(), picked }
        }
    }
}
