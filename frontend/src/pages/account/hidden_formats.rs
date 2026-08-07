//! "Hidden formats" account card: which formats the caller's landing All
//! Books view excludes. Checkbox list over the shared known-format vocabulary
//! (plus any saved token the list doesn't know, so it can always be
//! un-hidden); saving refreshes the app-wide `CurrentUser` context so the
//! landing page refetches with the new exclusion without a reload.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use omnibus_shared::KNOWN_LIBRARY_FORMATS;

use crate::components::credential_card::credential_status_message;
use crate::data;

/// The hidden-formats preference card (Settings → Account).
#[cfg(not(feature = "mobile"))]
#[component]
pub(crate) fn HiddenFormatsCard() -> Element {
    let mut selected = use_signal(BTreeSet::<String>::new);
    let mut seeded = use_signal(|| false);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    // Seed the selection once from the resolved viewer. Starts empty on SSR
    // and the first WASM paint (rule 07) and reconciles post-mount; the
    // `seeded` guard keeps a later context refresh (our own save) from
    // clobbering in-progress edits.
    let viewer = crate::use_current_user_summary();
    use_effect(move || {
        if *seeded.peek() {
            return;
        }
        if let Some(user) = viewer() {
            selected.set(user.hidden_formats.into_iter().collect());
            seeded.set(true);
        }
    });

    // Captured before the async save: `use_current_user` is a hook and can
    // only run during render.
    let mut viewer_slot = crate::use_current_user().0;
    let on_save = move |_| {
        let formats: Vec<String> = selected.peek().iter().cloned().collect();
        in_flight.set(true);
        msg.set(None);
        spawn(async move {
            match data::set_hidden_formats("", formats).await {
                Ok(()) => {
                    // Refresh the app-wide viewer so the landing `fetch_key`
                    // (which folds in `hidden_formats`) refetches at once.
                    if let Ok(resolved) = data::current_user().await {
                        viewer_slot.set(Some(resolved));
                    }
                    msg_is_error.set(false);
                    msg.set(Some("Hidden formats saved.".to_string()));
                }
                Err(e) => {
                    msg_is_error.set(true);
                    msg.set(Some(e.to_string()));
                }
            }
            in_flight.set(false);
        });
    };

    // Known formats first, then any saved token outside the list.
    let unknown_saved: Vec<String> = selected
        .read()
        .iter()
        .filter(|f| !KNOWN_LIBRARY_FORMATS.contains(&f.as_str()))
        .cloned()
        .collect();
    let current = selected.read().clone();

    rsx! {
        section { class: "card", "data-testid": "account-hidden-formats-card",
            h2 { "Hidden formats" }
            p { class: "subtitle",
                "Hide formats from your All Books view. A book stays visible while "
                "it has any format you haven't hidden; other readers are unaffected."
            }
            div { class: "settings-field hidden-formats-list",
                for fmt in KNOWN_LIBRARY_FORMATS.iter().map(|f| f.to_string()) {
                    {format_toggle(&fmt, current.contains(&fmt), selected)}
                }
                for fmt in unknown_saved {
                    {format_toggle(&fmt, true, selected)}
                }
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn",
                    disabled: in_flight(),
                    "data-testid": "hidden-formats-save",
                    onclick: on_save,
                    "Save"
                }
            }
            {credential_status_message("hidden-formats-status", msg().as_deref(), msg_is_error())}
        }
    }
}

/// One labelled checkbox row; toggling edits the shared selection set.
#[cfg(not(feature = "mobile"))]
fn format_toggle(fmt: &str, checked: bool, mut selected: Signal<BTreeSet<String>>) -> Element {
    let fmt = fmt.to_string();
    let toggle_fmt = fmt.clone();
    rsx! {
        label { class: "hidden-format-row", r#for: "hidden-format-{fmt}",
            input {
                r#type: "checkbox",
                id: "hidden-format-{fmt}",
                "data-testid": "hidden-format-toggle-{fmt}",
                checked,
                onchange: move |e| {
                    let mut set = selected.write();
                    if e.checked() {
                        set.insert(toggle_fmt.clone());
                    } else {
                        set.remove(&toggle_fmt);
                    }
                },
            }
            span { class: "mono", "{fmt}" }
        }
    }
}
