//! Note composer for a highlight. A small centered card (with scrim) holding
//! a markdown-capable textarea; saving persists via `update_highlight_note`
//! and reconciles the in-memory highlights list through `on_saved`.

use dioxus::prelude::*;
use omnibus_shared::Highlight;

use super::drawer_shell::ReaderScrim;

#[component]
pub(super) fn NoteComposer(
    highlight: Highlight,
    on_saved: EventHandler<(i64, Option<String>)>,
    on_close: EventHandler<()>,
) -> Element {
    let id = highlight.id;
    let server_url = crate::contexts::use_server_url();
    let mut draft = use_signal(|| highlight.note.clone().unwrap_or_default());
    let quote = highlight
        .text
        .clone()
        .unwrap_or_else(|| "your highlight".to_string());

    let on_save = move |_| {
        let text = draft.peek().trim().to_string();
        let note = if text.is_empty() { None } else { Some(text) };
        let note_for_event = note.clone();
        let server_url = server_url.clone();
        spawn(async move {
            if crate::data::update_highlight_note(&server_url, id, note.clone())
                .await
                .is_ok()
            {
                on_saved.call((id, note_for_event));
                on_close.call(());
            }
        });
    };

    rsx! {
        ReaderScrim { onclick: move |_| on_close.call(()) }
        div { class: "rd-note-composer", "data-testid": "reader-note-composer",
            onclick: move |e: MouseEvent| e.stop_propagation(),
            div { class: "rd-note-quote", "\u{201c}{quote}\u{201d}" }
            textarea {
                class: "rd-note-input",
                "data-testid": "reader-note-input",
                placeholder: "Add a note\u{2026} (markdown ok)",
                autofocus: true,
                value: "{draft}",
                oninput: move |e| draft.set(e.value()),
            }
            div { class: "rd-note-actions",
                span { class: "rd-note-hint", "markdown ok" }
                div { class: "rd-note-buttons",
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn primary sm",
                        r#type: "button",
                        "data-testid": "reader-note-save",
                        onclick: on_save,
                        "Save note"
                    }
                }
            }
        }
    }
}
