//! Admin-only "format conversion" card: view the resolved Calibre
//! `ebook-convert` command and override it with an explicit path. Calibre is
//! optional, so an unavailable binary renders as "Not detected" rather than an
//! error. Loads the status on mount; SSR/first paint start unresolved
//! (rule 07).

use dioxus::prelude::*;
use omnibus_shared::{EbookConvertStatus, DEFAULT_EBOOK_CONVERT_BIN};

use crate::components::credential_card::{credential_status_line, credential_status_message};
use crate::{data, use_server_url};

/// The input's placeholder: the resolved command once the status lands, else
/// what the backend actually falls back to — the bare name resolved on `$PATH`,
/// not a distro-specific absolute path.
fn placeholder_for(status: Option<&EbookConvertStatus>) -> String {
    status.map_or_else(|| DEFAULT_EBOOK_CONVERT_BIN.to_string(), |s| s.path.clone())
}

/// Whether a saved settings override (rather than the env var or `$PATH`) is in
/// force — the only case where "Use auto-detection" has anything to clear.
fn is_overridden(status: Option<&EbookConvertStatus>) -> bool {
    status.is_some_and(|s| s.source == "settings")
}

/// Detail line beside the status dot. Empty until the status lands, so the
/// unresolved card falls back to the caller's "Not detected" label.
fn status_detail(status: Option<&EbookConvertStatus>) -> String {
    status.map_or_else(String::new, |s| {
        format!("Detected \u{00b7} {} \u{00b7} {}", s.source, s.path)
    })
}

/// Message and error flag for a successful save: a path the server saved but
/// can't run is a warning, not a success.
fn save_outcome_message(available: bool) -> (String, bool) {
    if available {
        ("ebook-convert path saved.".to_string(), false)
    } else {
        (
            "Path saved, but ebook-convert is not runnable there.".to_string(),
            true,
        )
    }
}

/// The five signals [`EbookConvertField`] threads through its
/// load/save/clear handlers, bundled so each helper below takes one named
/// argument instead of five positional `Signal`s.
#[derive(Clone, Copy)]
struct EbookConvertFieldSignals {
    status: Signal<Option<EbookConvertStatus>>,
    path_input: Signal<String>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    in_flight: Signal<bool>,
}

/// Load the resolved `ebook-convert` status on mount.
fn use_load_ebook_convert_status(
    server_url: String,
    mut status: Signal<Option<EbookConvertStatus>>,
) {
    use_effect(move || {
        let url = server_url.clone();
        spawn(async move {
            if let Ok(s) = data::get_ebook_convert(&url).await {
                status.set(Some(s));
            }
        });
    });
}

/// Save handler body: empty input is a no-op with a hint — reverting to
/// auto-detection is the "Use auto-detection" button's job.
async fn save_ebook_convert_path(url: String, mut sigs: EbookConvertFieldSignals) {
    let value = (sigs.path_input)().trim().to_string();
    if value.is_empty() {
        sigs.msg
            .set(Some("Enter a path to ebook-convert to save.".to_string()));
        sigs.msg_is_error.set(true);
        return;
    }
    sigs.in_flight.set(true);
    match data::save_ebook_convert(&url, Some(value)).await {
        Ok(s) => {
            let (text, is_error) = save_outcome_message(s.available);
            sigs.status.set(Some(s));
            sigs.path_input.set(String::new());
            sigs.msg.set(Some(text));
            sigs.msg_is_error.set(is_error);
        }
        Err(_) => {
            sigs.msg
                .set(Some("Failed to save ebook-convert path.".to_string()));
            sigs.msg_is_error.set(true);
        }
    }
    sigs.in_flight.set(false);
}

/// Clear handler body.
async fn clear_ebook_convert_path(url: String, mut sigs: EbookConvertFieldSignals) {
    sigs.in_flight.set(true);
    match data::save_ebook_convert(&url, None).await {
        Ok(s) => {
            sigs.status.set(Some(s));
            sigs.path_input.set(String::new());
            sigs.msg.set(Some(
                "Override cleared \u{2014} using auto-detection.".to_string(),
            ));
            sigs.msg_is_error.set(false);
        }
        Err(_) => {
            sigs.msg
                .set(Some("Failed to clear ebook-convert path.".to_string()));
            sigs.msg_is_error.set(true);
        }
    }
    sigs.in_flight.set(false);
}

/// Admin field to override the `ebook_convert_path` setting. Clearing falls
/// back to `OMNIBUS_EBOOK_CONVERT_PATH`, then `ebook-convert` on `$PATH`.
#[component]
pub fn EbookConvertField() -> Element {
    let server_url = use_server_url();
    let sigs = EbookConvertFieldSignals {
        status: use_signal(|| None),
        path_input: use_signal(String::new),
        msg: use_signal(|| None::<String>),
        msg_is_error: use_signal(|| false),
        in_flight: use_signal(|| false),
    };
    let mut path_input = sigs.path_input;
    use_load_ebook_convert_status(server_url.clone(), sigs.status);

    let save_url = server_url.clone();
    let on_save = move |_| {
        spawn(save_ebook_convert_path(save_url.clone(), sigs));
    };
    let clear_url = server_url.clone();
    let on_clear = move |_| {
        spawn(clear_ebook_convert_path(clear_url.clone(), sigs));
    };

    let msg = sigs.msg;
    let msg_is_error = sigs.msg_is_error;
    let in_flight = sigs.in_flight;
    let st = (sigs.status)();
    let available = st.as_ref().is_some_and(|s| s.available);
    let overridden = is_overridden(st.as_ref());
    let placeholder = placeholder_for(st.as_ref());
    let detail = status_detail(st.as_ref());

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

#[cfg(test)]
mod tests;
