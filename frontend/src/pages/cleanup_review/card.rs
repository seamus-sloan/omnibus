//! The suggestion card and the decide bar under it.
//!
//! The card's shape is one idea: show what was scanned, show what would
//! replace it, and let the reviewer type over the replacement. Only a
//! book-title rename can carry an edit today — `apply_book_title_override`
//! takes an arbitrary title, while the taxonomy kinds have no rename
//! primitive — so every other kind renders the same anatomy with the
//! surviving value static rather than offering an edit the apply path would
//! refuse.

use dioxus::prelude::*;
use omnibus_shared::{CleanupAction, CleanupKind, SuggestionCard};

/// Whether this card's proposed value is the reviewer's to rewrite.
pub fn is_editable(card: &SuggestionCard) -> bool {
    card.kind == CleanupKind::BookTitle && card.action == CleanupAction::Rename
}

/// The value the card opens with — the detector's proposal where there is
/// one, the surviving name otherwise.
pub fn initial_value(card: &SuggestionCard) -> String {
    match card.action {
        CleanupAction::Rename => card.secondary_name.clone().unwrap_or_default(),
        _ => card.primary_name.clone(),
    }
}

/// One suggestion: the action sentence, the scanned value struck through, and
/// the proposed value beside it.
#[component]
pub fn SuggestionCardView(
    card: SuggestionCard,
    draft: Signal<String>,
    typing: Signal<bool>,
    on_commit: EventHandler<()>,
) -> Element {
    let editable = is_editable(&card);
    let edited = draft() != initial_value(&card);
    let books = if card.book_count == 1 {
        "1 book".to_string()
    } else {
        format!("{} books", card.book_count)
    };
    rsx! {
        section { class: "card crx-card", "data-testid": "cleanup-card",
            p { class: "crx-action-line", "data-testid": "cleanup-card-action",
                "{action_sentence(card.kind, card.action)}"
                span { class: "crx-detected mono", "data-testid": "cleanup-card-books", "{books} affected" }
            }
            div { class: "crx-proposal",
                div {
                    span { class: "crx-side-label", "{scanned_label(card.action)}" }
                    ScannedValue { card: card.clone() }
                }
                div { class: "crx-arrow", "aria-hidden": "true", "\u{2192}" }
                div { class: "crx-proposed-wrap",
                    span { class: "crx-side-label",
                        "{proposed_label(card.action)}"
                        if edited {
                            span { class: "crx-edited-tag", "data-testid": "cleanup-edited-tag", "edited" }
                        }
                    }
                    if card.action == CleanupAction::Split {
                        SplitParts { parts: card.proposed_parts.clone() }
                    } else {
                        ProposedValue { card: card.clone(), editable, draft, typing, on_commit }
                    }
                }
            }
            if let Some(photo) = card.photo_url.clone() {
                img {
                    class: "cleanup-card-photo",
                    src: "{photo}",
                    alt: "",
                    "data-testid": "cleanup-card-photo",
                }
            }
            p { class: "crx-hint", "data-testid": "cleanup-hint",
                if editable {
                    "Type over the proposal to accept your own wording — "
                } else {
                    // Deliberately not "the surviving record": this arm also
                    // covers a split, whose proposal is a set of parts.
                    "Editing the proposal isn't wired up for this kind yet — "
                }
                kbd { "Y" }
                " accepts, "
                kbd { "N" }
                " rejects, "
                kbd { "Space" }
                " skips it for this pass."
            }
        }
    }
}

/// The proposed value: an input the reviewer owns for a book title, a static
/// field for every other kind.
///
/// Both render the same element type with the same class, so the two states
/// read as one control rather than swapping layouts — and the static arm is
/// genuinely static, not a disabled input the reviewer would keep clicking.
#[component]
fn ProposedValue(
    card: SuggestionCard,
    editable: bool,
    draft: Signal<String>,
    typing: Signal<bool>,
    on_commit: EventHandler<()>,
) -> Element {
    if !editable {
        return rsx! {
            div { class: "crx-proposed crx-proposed-static", "data-testid": "cleanup-card-proposed",
                "{draft}"
            }
        };
    }
    rsx! {
        input {
            class: "crx-proposed",
            "data-testid": "cleanup-proposed-input",
            "aria-label": "Proposed title",
            autocapitalize: "off",
            autocorrect: "off",
            spellcheck: "false",
            value: "{draft}",
            oninput: move |evt| draft.set(evt.value()),
            // The card owns Y/N/Space, so they have to stop being hotkeys
            // while the reviewer is typing into this field — otherwise the
            // letter y in a title decides the card instead of landing in it.
            onfocusin: move |_| typing.set(true),
            onfocusout: move |_| typing.set(false),
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Enter {
                    evt.prevent_default();
                    on_commit.call(());
                }
            },
        }
    }
}

/// The Accept / Reject / Skip row, with the Accept label naming what accepting
/// would actually write.
#[component]
pub fn DecideBar(
    edited: bool,
    editable: bool,
    in_flight: bool,
    on_accept: EventHandler<()>,
    on_reject: EventHandler<()>,
    on_skip: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "crx-decide", "data-testid": "cleanup-review-actions",
            button {
                class: "btn primary",
                "data-testid": "cleanup-accept",
                disabled: in_flight,
                onclick: move |_| on_accept.call(()),
                if edited { "Accept edited title (Y)" } else { "Accept (Y)" }
            }
            button {
                class: "btn",
                "data-testid": "cleanup-reject",
                disabled: in_flight,
                onclick: move |_| on_reject.call(()),
                "Reject (N)"
            }
            button {
                class: "btn ghost",
                "data-testid": "cleanup-skip",
                onclick: move |_| on_skip.call(()),
                "Skip (Space)"
            }
            span { class: "crx-decide-spacer" }
            span { class: "crx-decide-note mono", "data-testid": "cleanup-decide-note",
                "{decide_note(edited, editable)}"
            }
        }
    }
}

/// What the note beside the buttons says accepting will do.
fn decide_note(edited: bool, editable: bool) -> &'static str {
    if edited {
        "your wording replaces the proposal"
    } else if editable {
        "accepting applies at once \u{b7} undo from Logs"
    } else {
        "accepting applies at once"
    }
}

/// The label over the value being replaced.
fn scanned_label(action: CleanupAction) -> &'static str {
    match action {
        CleanupAction::Rename => "Scanned title",
        CleanupAction::Split => "Scanned tag",
        CleanupAction::Delete => "Record",
        CleanupAction::Merge => "Merging away",
    }
}

/// The label over the value that would replace it.
fn proposed_label(action: CleanupAction) -> &'static str {
    match action {
        CleanupAction::Rename => "Proposed title",
        CleanupAction::Split => "Splits into",
        CleanupAction::Delete => "Becomes",
        CleanupAction::Merge => "Surviving name",
    }
}

/// The value being replaced. A merge folds away one record or several, and
/// every one of them is named — the card knows them all, so summarising them
/// as a count would be inventing a number the reader can't check.
#[component]
fn ScannedValue(card: SuggestionCard) -> Element {
    if card.action == CleanupAction::Merge {
        return rsx! {
            ul { class: "crx-merge-list", "data-testid": "cleanup-card-scanned",
                for name in card.source_names.iter() {
                    li { key: "{name}", class: "crx-merge-row",
                        span { class: "crx-merge-name crx-strike", "{name}" }
                    }
                }
            }
        };
    }
    rsx! {
        div { class: "crx-current crx-strike", "data-testid": "cleanup-card-scanned",
            "{card.primary_name}"
        }
    }
}

/// What a split would write: one chip per atom, in parse order.
#[component]
fn SplitParts(parts: Vec<String>) -> Element {
    rsx! {
        div { class: "crx-parts", "data-testid": "cleanup-card-proposed",
            for part in parts.iter() {
                span { key: "{part}", class: "chip crx-part", "{part}" }
            }
        }
    }
}

/// The one-line description of what accepting this card does.
pub fn action_sentence(kind: CleanupKind, action: CleanupAction) -> &'static str {
    match (kind, action) {
        (CleanupKind::Author, CleanupAction::Merge) => "Merge these authors into one.",
        (CleanupKind::Author, CleanupAction::Delete) => "Delete this author record.",
        (CleanupKind::Series, CleanupAction::Merge) => "Merge these series into one.",
        (CleanupKind::Tag, CleanupAction::Merge) => "Merge these tags into one.",
        (CleanupKind::Tag, CleanupAction::Split) => "Split this tag into its parts.",
        (CleanupKind::BookTitle, CleanupAction::Rename) => "Adopt the normalized title.",
        _ => "Apply this cleanup.",
    }
}
