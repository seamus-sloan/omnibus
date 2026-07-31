//! Manual ISBN entry — the scanner's always-available fallback. A text input
//! (so a hardware keyboard, and a scanner gun emulating one, still works) over
//! an on-screen keypad, with a 13-slot progress strip. Visible text is the
//! E2E selector contract — keep it stable (rule 04).

use dioxus::prelude::*;

use super::{clean_isbn, isbn_rejection};

/// Slots in the progress strip: an ISBN-13, the form every barcode carries.
/// A valid ISBN-10 simply leaves the last three dark.
const ISBN_SLOTS: usize = 13;

/// The keypad's face, row-major. `X` is the ISBN-10 check digit, the one
/// non-digit a valid ISBN can end with.
const KEYS: [&str; 12] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "X", "0", "\u{232b}",
];

/// Manual ISBN entry. Submits through `on_resolve` once the check digit
/// validates; `on_scan` hands the flow back to the camera.
#[component]
pub(super) fn EntryScreen(
    isbn: Signal<String>,
    busy: Signal<bool>,
    on_resolve: EventHandler<String>,
    on_scan: EventHandler<()>,
) -> Element {
    let mut isbn = isbn;
    let cleaned = clean_isbn(&isbn());
    // Gate the submit on the shared validator rather than on length alone, so
    // a transposed digit is caught here instead of a round-trip later (AC3).
    let ready = isbn_rejection(&cleaned).is_none();

    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-entry",
            h1 { "Enter the ISBN" }
            p { class: "subtitle",
                "It's the 13-digit number under the barcode on the back cover."
            }
            form {
                class: "settings-form",
                "data-testid": "check-in-form",
                onsubmit: move |evt| {
                    evt.prevent_default();
                    on_resolve.call(isbn());
                },
                div { class: "settings-field",
                    label { r#for: "check-in-isbn", "ISBN" }
                    input {
                        id: "check-in-isbn",
                        "data-testid": "check-in-isbn",
                        r#type: "text",
                        inputmode: "numeric",
                        autocomplete: "off",
                        placeholder: "9780000000000",
                        value: "{isbn}",
                        disabled: busy(),
                        oninput: move |e| isbn.set(e.value()),
                    }
                }
                IsbnProgress { cleaned: cleaned.clone() }
                Keypad { isbn, busy }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn primary",
                        disabled: busy() || !ready,
                        "data-testid": "check-in-submit",
                        "Find book"
                    }
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: busy(),
                        "data-testid": "check-in-scan-instead",
                        onclick: move |_| on_scan.call(()),
                        "Scan a barcode instead"
                    }
                }
            }
        }
    }
}

/// The 13-slot fill strip. Each slot shows its character once typed, so a
/// misread digit is visible without counting along the input.
#[component]
fn IsbnProgress(cleaned: String) -> Element {
    let chars: Vec<char> = cleaned.chars().collect();
    rsx! {
        div {
            class: "isbn-progress",
            "data-testid": "check-in-isbn-progress",
            "data-filled": "{chars.len()}",
            "aria-hidden": "true",
            for i in 0..ISBN_SLOTS {
                span {
                    key: "{i}",
                    class: if i < chars.len() { "isbn-slot filled" } else { "isbn-slot" },
                    {chars.get(i).map(|c| c.to_string()).unwrap_or_default()}
                }
            }
        }
    }
}

/// On-screen numeric keypad driving the same `isbn` signal as the text input.
#[component]
fn Keypad(isbn: Signal<String>, busy: Signal<bool>) -> Element {
    let mut isbn = isbn;
    rsx! {
        div { class: "isbn-keypad", "data-testid": "check-in-keypad",
            for key in KEYS {
                button {
                    key: "{key}",
                    r#type: "button",
                    class: "isbn-key",
                    disabled: busy(),
                    "data-testid": "check-in-key-{key}",
                    "aria-label": if key == "\u{232b}" { "Delete" } else { "{key}" },
                    onclick: move |_| isbn.set(apply_key(&isbn(), key)),
                    "{key}"
                }
            }
        }
    }
}

/// Apply one keypad press to the current entry: backspace pops the last
/// character, anything else appends up to the 13-slot cap.
pub(super) fn apply_key(current: &str, key: &str) -> String {
    let mut cleaned = clean_isbn(current);
    if key == "\u{232b}" {
        cleaned.pop();
    } else if cleaned.chars().count() < ISBN_SLOTS {
        cleaned.push_str(key);
    }
    cleaned
}
