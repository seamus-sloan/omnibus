//! Admin-only "format conversion" card: view the resolved Calibre
//! `ebook-convert` command and override it with an explicit path. Calibre is
//! optional, so an unavailable binary renders as "Not detected" rather than an
//! error. Loads the status on mount; SSR/first paint start unresolved
//! (rule 07).

use dioxus::prelude::*;
use omnibus_shared::EbookConvertStatus;

use crate::components::credential_card::{credential_status_line, credential_status_message};
use crate::{data, use_server_url};

/// Admin field to override the `ebook_convert_path` setting. Clearing falls
/// back to `OMNIBUS_EBOOK_CONVERT_PATH`, then `ebook-convert` on `$PATH`.
#[component]
pub fn EbookConvertField() -> Element {
    let server_url = use_server_url();
    let mut status: Signal<Option<EbookConvertStatus>> = use_signal(|| None);
    let mut path_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    let load_url = server_url.clone();
    use_effect(move || {
        let url = load_url.clone();
        spawn(async move {
            if let Ok(s) = data::get_ebook_convert(&url).await {
                status.set(Some(s));
            }
        });
    });

    let save_url = server_url.clone();
    let on_save = move |_| {
        let value = path_input().trim().to_string();
        // Empty Save is a no-op with a hint — reverting to auto-detection is
        // the "Use auto-detection" button's job.
        if value.is_empty() {
            msg.set(Some("Enter a path to ebook-convert to save.".to_string()));
            msg_is_error.set(true);
            return;
        }
        let url = save_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::save_ebook_convert(&url, Some(value)).await {
                Ok(s) => {
                    let detected = s.available;
                    status.set(Some(s));
                    path_input.set(String::new());
                    msg.set(Some(if detected {
                        "ebook-convert path saved.".to_string()
                    } else {
                        "Path saved, but ebook-convert is not runnable there.".to_string()
                    }));
                    msg_is_error.set(!detected);
                }
                Err(_) => {
                    msg.set(Some("Failed to save ebook-convert path.".to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let clear_url = server_url.clone();
    let on_clear = move |_| {
        let url = clear_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::save_ebook_convert(&url, None).await {
                Ok(s) => {
                    status.set(Some(s));
                    path_input.set(String::new());
                    msg.set(Some(
                        "Override cleared \u{2014} using auto-detection.".to_string(),
                    ));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to clear ebook-convert path.".to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let st = status();
    let available = st.as_ref().map(|s| s.available).unwrap_or(false);
    let overridden = st.as_ref().map(|s| s.source == "settings").unwrap_or(false);
    let placeholder = st
        .as_ref()
        .map(|s| s.path.clone())
        .unwrap_or_else(|| "/usr/bin/ebook-convert".to_string());
    let detail = st
        .as_ref()
        .map(|s| format!("Detected \u{00b7} {} \u{00b7} {}", s.source, s.path))
        .unwrap_or_default();

    rsx! {
        section { class: "card", "data-testid": "ebook-convert-card",
            h2 { "Format conversion" }
            p { class: "subtitle",
                "Omnibus converts between ebook formats with Calibre\u{2019}s ebook-convert. "
                "Calibre is optional \u{2014} without it, conversion stays disabled and "
                "everything else works as usual."
            }
            div { class: "settings-field",
                label { r#for: "ebook-convert-path", "ebook-convert Path" }
                input {
                    r#type: "text",
                    id: "ebook-convert-path",
                    name: "ebook_convert_path",
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    placeholder: "{placeholder}",
                    value: "{path_input}",
                    oninput: move |e| path_input.set(e.value()),
                }
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn",
                    disabled: in_flight(),
                    "data-testid": "ebook-convert-save",
                    onclick: on_save,
                    "Save"
                }
                if overridden {
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: in_flight(),
                        "data-testid": "ebook-convert-clear",
                        onclick: on_clear,
                        "Use auto-detection"
                    }
                }
            }
            {credential_status_line("ebook-convert-status", available, &detail, "Not detected")}
            {credential_status_message("ebook-convert-save-status", msg().as_deref(), msg_is_error())}
        }
    }
}
