//! Admin-only "server-wide secret key" card for the settings page. One
//! parameterized component ([`SecretKeyField`] over [`SecretKeyKind`]) behind
//! both the Hardcover and Google Books keys — they differ only in labels,
//! testids, and transport. Loads the masked status on mount; the raw key never
//! comes back to the client. SSR/first-paint start unconfigured (rule 07).

use dioxus::prelude::*;
use omnibus_shared::{GoogleBooksKeyStatus, HardcoverKeyStatus};

use crate::components::credential_card::{credential_status_line, credential_status_message};
use crate::data::DataError;
use crate::{data, use_server_url};

/// Which server-wide secret this card manages. `Copy` so every async handler
/// can capture it.
#[derive(Clone, Copy, PartialEq)]
pub enum SecretKeyKind {
    /// The Hardcover token behind F3.3 "Readers also enjoyed" suggestions.
    Hardcover,
    /// The Google Books key that keeps ISBN check-ins resolving past the shared
    /// anonymous quota.
    GoogleBooks,
}

/// Masked key status, normalized across the two wire types (which are
/// structurally identical) so the component works on one shape.
#[derive(Clone)]
struct KeyStatus {
    configured: bool,
    masked: Option<String>,
    source: String,
}

impl From<HardcoverKeyStatus> for KeyStatus {
    fn from(s: HardcoverKeyStatus) -> Self {
        Self {
            configured: s.configured,
            masked: s.masked,
            source: s.source,
        }
    }
}

impl From<GoogleBooksKeyStatus> for KeyStatus {
    fn from(s: GoogleBooksKeyStatus) -> Self {
        Self {
            configured: s.configured,
            masked: s.masked,
            source: s.source,
        }
    }
}

impl SecretKeyKind {
    /// Read the masked status via the matching transport.
    async fn fetch(self, url: &str) -> Result<KeyStatus, DataError> {
        match self {
            Self::Hardcover => data::get_hardcover_key(url).await.map(Into::into),
            Self::GoogleBooks => data::get_google_books_key(url).await.map(Into::into),
        }
    }

    /// Save (`Some`) or clear (`None`) the key; returns the new masked status.
    async fn save(self, url: &str, key: Option<String>) -> Result<KeyStatus, DataError> {
        match self {
            Self::Hardcover => data::set_hardcover_key(url, key).await.map(Into::into),
            Self::GoogleBooks => data::set_google_books_key(url, key).await.map(Into::into),
        }
    }

    /// Human noun used in the status messages ("Hardcover key saved.").
    fn noun(self) -> &'static str {
        match self {
            Self::Hardcover => "Hardcover",
            Self::GoogleBooks => "Google Books",
        }
    }

    /// Card heading.
    fn heading(self) -> &'static str {
        match self {
            Self::Hardcover => "Suggestions",
            Self::GoogleBooks => "Book metadata lookup",
        }
    }

    /// Card subtitle.
    fn subtitle(self) -> &'static str {
        match self {
            Self::Hardcover => "Connect Hardcover to power \u{201c}Readers also enjoyed\u{201d}.",
            Self::GoogleBooks => {
                "Add a Google Books API key so ISBN check-ins keep resolving past the shared anonymous quota."
            }
        }
    }

    /// `<label>` text (also the Playwright `getByLabel` selector).
    fn label(self) -> &'static str {
        match self {
            Self::Hardcover => "Hardcover API Key",
            Self::GoogleBooks => "Google Books API Key",
        }
    }

    /// `<input>` id (and `<label for=…>` target).
    fn input_id(self) -> &'static str {
        match self {
            Self::Hardcover => "hardcover-key",
            Self::GoogleBooks => "google-books-key",
        }
    }

    /// `<input>` name.
    fn input_name(self) -> &'static str {
        match self {
            Self::Hardcover => "hardcover_api_key",
            Self::GoogleBooks => "google_books_api_key",
        }
    }

    /// Placeholder shown before a masked value loads.
    fn placeholder_default(self) -> &'static str {
        match self {
            Self::Hardcover => "hc_live_\u{2026}",
            Self::GoogleBooks => "AIza\u{2026}",
        }
    }

    /// testid prefix for the card and its controls; each control appends its
    /// own suffix to keep the existing selector contract exact.
    fn testid(self) -> &'static str {
        match self {
            Self::Hardcover => "hardcover",
            Self::GoogleBooks => "google-books",
        }
    }
}

/// Admin field to set/clear one server-wide secret key. Loads the masked status
/// on mount; the raw key is never read back to the client.
#[component]
pub fn SecretKeyField(kind: SecretKeyKind) -> Element {
    let server_url = use_server_url();
    let mut status: Signal<Option<KeyStatus>> = use_signal(|| None);
    let mut key_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    let load_url = server_url.clone();
    use_effect(move || {
        let url = load_url.clone();
        spawn(async move {
            if let Ok(s) = kind.fetch(&url).await {
                status.set(Some(s));
            }
        });
    });

    let save_url = server_url.clone();
    let on_save = move |_| {
        let value = key_input().trim().to_string();
        // Empty Save is a no-op with a hint — clearing is the "Clear" button's
        // job, so we never silently wipe the key and report "saved".
        if value.is_empty() {
            msg.set(Some(format!("Enter a {} key to save.", kind.noun())));
            msg_is_error.set(true);
            return;
        }
        let url = save_url.clone();
        in_flight.set(true);
        spawn(async move {
            match kind.save(&url, Some(value)).await {
                Ok(s) => {
                    status.set(Some(s));
                    key_input.set(String::new());
                    msg.set(Some(format!("{} key saved.", kind.noun())));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some(format!("Failed to save {} key.", kind.noun())));
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
            match kind.save(&url, None).await {
                Ok(s) => {
                    status.set(Some(s));
                    key_input.set(String::new());
                    msg.set(Some(format!("{} key cleared.", kind.noun())));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some(format!("Failed to clear {} key.", kind.noun())));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let st = status();
    let configured = st.as_ref().map(|s| s.configured).unwrap_or(false);
    let placeholder = st
        .as_ref()
        .and_then(|s| s.masked.clone())
        .unwrap_or_else(|| kind.placeholder_default().to_string());
    let testid = kind.testid();
    let detail = st
        .as_ref()
        .map(|s| {
            format!(
                "Connected \u{00b7} {} \u{00b7} {}",
                s.source,
                s.masked.clone().unwrap_or_default()
            )
        })
        .unwrap_or_default();

    rsx! {
        section { class: "card", "data-testid": "{testid}-key-card",
            h2 { "{kind.heading()}" }
            p { class: "subtitle", "{kind.subtitle()}" }
            div { class: "settings-field",
                label { r#for: kind.input_id(), "{kind.label()}" }
                input {
                    r#type: "password",
                    id: kind.input_id(),
                    name: kind.input_name(),
                    // Server-wide secret: keep password managers / autofill /
                    // spellcheck from storing or mangling it.
                    autocomplete: "off",
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    placeholder: "{placeholder}",
                    value: "{key_input}",
                    oninput: move |e| key_input.set(e.value()),
                }
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn",
                    disabled: in_flight(),
                    "data-testid": "{testid}-save",
                    onclick: on_save,
                    "Save"
                }
                if configured {
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: in_flight(),
                        "data-testid": "{testid}-clear",
                        onclick: on_clear,
                        "Clear"
                    }
                }
            }
            {credential_status_line(&format!("{testid}-status"), configured, &detail, "Not connected")}
            {credential_status_message(&format!("{testid}-key-status"), msg().as_deref(), msg_is_error())}
        }
    }
}
