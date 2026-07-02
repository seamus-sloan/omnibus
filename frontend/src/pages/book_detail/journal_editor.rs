//! Live markdown editor glue for the journal composer.
//!
//! Bridges the Dioxus components to the hand-rolled `contenteditable` editor in
//! the co-located `journal_editor.js`: it ships the JS module over the eval
//! channel, exposes `enhance` / `command` / `insert` actions, renders the
//! formatting toolbar, and builds the insert-from-highlights blockquote. The
//! rsx renders a plain textarea on every target (so SSR and the first WASM
//! paint match, per rule 07); web progressively enhances it into a
//! contenteditable overlay from a post-mount `editor_enhance` call.

use dioxus::prelude::*;

/// The live-editor JS module (defines `window.OmnibusJournalEditor`, then
/// dispatches one action read from the Dioxus eval channel). Included as a
/// string and evaluated on demand; the module guards against re-definition so
/// repeated evals only run the trailing dispatcher.
#[cfg(feature = "web")]
const JOURNAL_EDITOR_JS: &str = include_str!("journal_editor.js");

/// Send one action to the live editor over the eval channel. Web-only — the
/// eval (and `serde_json`) are unavailable on SSR/native, so this is a no-op
/// there. The rsx renders a plain textarea on every target; non-web keeps that
/// textarea (the enhance dispatch here is a no-op), so mobile / SSR never
/// materialize the contenteditable overlay.
#[cfg(feature = "web")]
fn editor_dispatch(action: &str, editor_id: &str, mirror_id: &str, op: &str, a: &str, b: &str) {
    let eval = dioxus::document::eval(JOURNAL_EDITOR_JS);
    let _ = eval.send(serde_json::json!({
        "action": action, "editorId": editor_id, "mirrorId": mirror_id,
        "op": op, "a": a, "b": b, "text": a,
    }));
}

#[cfg(not(feature = "web"))]
fn editor_dispatch(
    _action: &str,
    _editor_id: &str,
    _mirror_id: &str,
    _op: &str,
    _a: &str,
    _b: &str,
) {
}

/// Progressively enhance a plain `<textarea id=mirror_id>` (rendered by rsx on
/// every target) into a live-editor pair: JS creates the `<div id=editor_id
/// contenteditable>` overlay, hides the textarea, and wires the pair together.
/// Called from the textarea's `onmounted` on every target — a no-op on non-web
/// where the eval channel is stubbed, so mobile falls back to the plain
/// textarea automatically.
///
/// `editor_testid` becomes the overlay div's `data-testid` (Playwright targets
/// it). `aria_label` / `placeholder` are copied onto the overlay so
/// accessibility and empty-state hints match the composer/edit contexts.
pub(crate) fn editor_enhance(
    editor_id: &str,
    mirror_id: &str,
    editor_testid: &str,
    aria_label: &str,
    placeholder: &str,
) {
    // op/a/b slots on the shared dispatch payload carry the enhance-specific
    // extras (testid / aria-label / placeholder) so the JS action can build the
    // overlay without a bespoke channel.
    editor_dispatch(
        "enhance",
        editor_id,
        mirror_id,
        editor_testid,
        aria_label,
        placeholder,
    );
}

/// Run a toolbar formatting command (`wrap` / `prefix` / `link`) on the live
/// editor's current selection.
pub(crate) fn editor_command(editor_id: &str, op: &str, a: &str, b: &str) {
    editor_dispatch("command", editor_id, "", op, a, b);
}

/// Insert `text` at the live editor's caret (used by insert-from-highlights).
/// `text` rides the same channel slot the JS reads for an `insert` action.
pub(crate) fn editor_insert(editor_id: &str, text: &str) {
    editor_dispatch("insert", editor_id, "", "", text, "");
}

/// One formatting button: visible `label`, accessible `title`, and the
/// `(op, a, b)` triple passed to [`editor_command`].
struct ToolbarButton {
    label: &'static str,
    title: &'static str,
    op: &'static str,
    a: &'static str,
    b: &'static str,
}

/// The composer formatting toolbar. Each button wraps/prefixes the live
/// editor's selection with the matching markdown so the persisted body stays
/// plain markdown. `target_id` is the id of the contenteditable the buttons act
/// on. Buttons `prevent_default` on mousedown so clicking one doesn't move
/// focus out of the editor and collapse the selection.
#[component]
pub(crate) fn BdJournalToolbar(target_id: String) -> Element {
    // Order mirrors the design's `InlineJournalEditor` toolbar. The image
    // button is omitted — embedded uploads (F5.3) haven't shipped.
    const BUTTONS: &[ToolbarButton] = &[
        ToolbarButton {
            label: "B",
            title: "Bold",
            op: "wrap",
            a: "**",
            b: "**",
        },
        ToolbarButton {
            label: "I",
            title: "Italic",
            op: "wrap",
            a: "*",
            b: "*",
        },
        ToolbarButton {
            label: "S",
            title: "Strikethrough",
            op: "wrap",
            a: "~~",
            b: "~~",
        },
        ToolbarButton {
            label: "H1",
            title: "Heading 1",
            op: "prefix",
            a: "# ",
            b: "",
        },
        ToolbarButton {
            label: "H2",
            title: "Heading 2",
            op: "prefix",
            a: "## ",
            b: "",
        },
        ToolbarButton {
            label: "\u{201C}",
            title: "Quote",
            op: "prefix",
            a: "> ",
            b: "",
        },
        ToolbarButton {
            label: "\u{2022}",
            title: "Bullet list",
            op: "prefix",
            a: "- ",
            b: "",
        },
        ToolbarButton {
            label: "1.",
            title: "Numbered list",
            op: "prefix",
            a: "1. ",
            b: "",
        },
        ToolbarButton {
            label: "[ ]",
            title: "Checklist",
            op: "prefix",
            a: "- [ ] ",
            b: "",
        },
        ToolbarButton {
            label: "{ }",
            title: "Inline code",
            op: "wrap",
            a: "`",
            b: "`",
        },
        ToolbarButton {
            label: "\u{1F517}",
            title: "Link",
            op: "link",
            a: "text",
            b: "https://",
        },
        ToolbarButton {
            label: "Spoiler",
            title: "Spoiler — blurred until clicked",
            op: "wrap",
            a: "||",
            b: "||",
        },
    ];

    rsx! {
        div { class: "bd-journal-toolbar", "data-testid": "journal-toolbar", role: "toolbar",
            for btn in BUTTONS.iter() {
                button {
                    r#type: "button",
                    class: "btn ghost sm bd-journal-tool",
                    title: "{btn.title}",
                    "aria-label": "{btn.title}",
                    onmousedown: move |e| e.prevent_default(),
                    onclick: {
                        let id = target_id.clone();
                        move |_| editor_command(&id, btn.op, btn.a, btn.b)
                    },
                    "{btn.label}"
                }
            }
        }
    }
}

/// Build the markdown blockquote inserted when a saved highlight is chosen:
/// every line of the passage is `> `-prefixed, followed by an attribution line.
pub(crate) fn highlight_blockquote(text: &str) -> String {
    let quoted = text
        .lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n{quoted}\n>\n> \u{2014} saved from highlights\n")
}
