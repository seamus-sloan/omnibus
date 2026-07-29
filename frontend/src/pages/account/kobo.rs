//! Kobo wireless-sync device card (web Account section).
//!
//! Lists the caller's registered Kobos with their re-displayable `api_endpoint`
//! URL (AC2), per-device Regenerate/Remove, and an "Add a Kobo" form. Signals
//! start empty so SSR and the first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::KoboDeviceView;

use crate::{data, use_server_url};

/// The `api_endpoint` a Kobo is pointed at: `{origin}/kobo/{token}`. The reader
/// appends `/v1/…` itself.
fn endpoint_url(server_url: &str, token: &str) -> String {
    format!("{}/kobo/{token}", server_url.trim_end_matches('/'))
}

/// Card body: device list + add form. Every mutation re-fetches the list so the
/// rendered state always matches the server.
#[component]
pub fn KoboDevicesCard() -> Element {
    let server_url = use_server_url();
    let mut devices = use_signal(Vec::<KoboDeviceView>::new);
    let mut name_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    let load_url = server_url.clone();
    use_effect(move || {
        let url = load_url.clone();
        spawn(async move {
            if let Ok(list) = data::list_kobo_devices(&url).await {
                devices.set(list);
            }
        });
    });

    let add_url = server_url.clone();
    let on_add = move |evt: Event<FormData>| {
        evt.prevent_default();
        let name = name_input().trim().to_string();
        if name.is_empty() {
            msg.set(Some("Enter a name for your Kobo.".to_string()));
            msg_is_error.set(true);
            return;
        }
        let url = add_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::create_kobo_device(&url, name).await {
                Ok(dev) => {
                    name_input.set(String::new());
                    devices.write().push(dev);
                    msg.set(Some(
                        "Kobo added. Paste its endpoint URL below.".to_string(),
                    ));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    // `use_callback` handles are `Copy`, so they survive being passed into each
    // row inside the `for` loop below (a plain closure would move on first use).
    let regen_url = server_url.clone();
    let on_regenerate = use_callback(move |id: i64| {
        let url = regen_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::regenerate_kobo_device(&url, id).await {
                Ok(updated) => {
                    if let Some(slot) = devices.write().iter_mut().find(|d| d.id == id) {
                        *slot = updated;
                    }
                    msg.set(Some(
                        "Token regenerated. Update the URL on your Kobo.".to_string(),
                    ));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    });

    let revoke_url = server_url.clone();
    let on_remove = use_callback(move |id: i64| {
        let url = revoke_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::revoke_kobo_device(&url, id).await {
                Ok(()) => {
                    devices.write().retain(|d| d.id != id);
                    msg.set(Some("Kobo removed.".to_string()));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    });

    let device_list = devices();

    rsx! {
        section { class: "card", "data-testid": "kobo-devices-card",
            h2 { "Kobo wireless sync" }
            p { class: "subtitle",
                "There is no on-device setting for this \u{2014} the endpoint is set "
                "by editing a config file on the Kobo over USB:"
            }
            ol { class: "subtitle kobo-setup-steps", "data-testid": "kobo-setup-steps",
                li {
                    "Give your Kobo a name and click "
                    b { "Add a Kobo" }
                }
                li {
                    "Copy the device's wireless sync endpoint URL. ("
                    code { "/kobo/<token>" }
                    ")"
                }
                li { "Connect the Kobo over USB," }
                li {
                    "Edit "
                    code { ".kobo/Kobo/Kobo eReader.conf" }
                    " and set "
                    code { "api_endpoint=" }
                    " under "
                    code { "[OneStoreServices]" }
                    " to "
                    code { "<your_omnibus_server_url>/kobo/<token>" }
                    "."
                }
                li { "Eject safely. Your next sync on the device talks to Omnibus." }
            }

            // Unconditional: the hazard applies the moment a URL is copied.
            div { class: "kobo-warning", "data-testid": "kobo-data-loss-warning",
                p { class: "kobo-warning-title", "First sync can erase your Kobo's highlights" }
                p {
                    "An improper sync can potentially wipe data on the Kobo like your "
                    "annotations, notes, bookmarks, reading progress, and books."
                }
                p {
                    "It is highly recommended to back up your device before attempting "
                    "a sync with one of these tools:"
                }
                ul { class: "kobo-warning-tools",
                    li {
                        a {
                            href: "https://github.com/seamus-sloan/kobo-backup#kobo-backup",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "seamus-sloan/kobo-backup"
                        }
                    }
                    li {
                        a {
                            href: "https://github.com/karlicoss/kobuddy#usage",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "karlicoss/kobuddy"
                        }
                    }
                }
                p {
                    "At minimum, copy "
                    code { ".kobo/KoboReader.sqlite" }
                    " off the device over USB."
                }
            }

            if device_list.is_empty() {
                p {
                    class: "settings-status",
                    "data-testid": "kobo-devices-empty",
                    "No Kobo registered yet."
                }
            } else {
                ul { class: "kobo-device-list", "data-testid": "kobo-device-list",
                    for dev in device_list.iter().cloned() {
                        KoboDeviceRow {
                            key: "{dev.id}",
                            device: dev.clone(),
                            endpoint: endpoint_url(&server_url, &dev.token),
                            disabled: in_flight(),
                            on_regenerate,
                            on_remove,
                        }
                    }
                }
            }

            form {
                id: "kobo-add-form",
                class: "settings-form",
                onsubmit: on_add,
                div { class: "settings-field",
                    label { r#for: "kobo-device-name", "Device name" }
                    input {
                        r#type: "text",
                        id: "kobo-device-name",
                        name: "kobo_device_name",
                        "data-testid": "kobo-device-name-input",
                        autocomplete: "off",
                        placeholder: "Kobo Clara",
                        maxlength: "100",
                        value: "{name_input}",
                        oninput: move |e| name_input.set(e.value()),
                    }
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        disabled: in_flight(),
                        "data-testid": "kobo-device-add",
                        "Add a Kobo"
                    }
                }
            }

            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "kobo-devices-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

/// One device row: label, the readonly endpoint URL (for copy), and the
/// Regenerate / Remove actions.
#[component]
fn KoboDeviceRow(
    device: KoboDeviceView,
    endpoint: String,
    disabled: bool,
    on_regenerate: Callback<i64>,
    on_remove: Callback<i64>,
) -> Element {
    let id = device.id;
    rsx! {
        li { class: "kobo-device-row", "data-testid": "kobo-device-row",
            div { class: "kobo-device-head",
                span { class: "kobo-device-name", "{device.name}" }
            }
            div { class: "settings-field",
                label { r#for: "kobo-endpoint-{id}", "Wireless sync endpoint" }
                input {
                    r#type: "text",
                    id: "kobo-endpoint-{id}",
                    class: "kobo-endpoint-url",
                    "data-testid": "kobo-endpoint-url",
                    readonly: true,
                    value: "{endpoint}",
                }
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled,
                    "data-testid": "kobo-device-regenerate",
                    onclick: move |_| on_regenerate.call(id),
                    "Regenerate"
                }
                button {
                    r#type: "button",
                    class: "btn ghost danger",
                    disabled,
                    "data-testid": "kobo-device-remove",
                    onclick: move |_| on_remove.call(id),
                    "Remove"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_joins_origin_and_token() {
        assert_eq!(
            endpoint_url("https://omni.example.com", "tok123"),
            "https://omni.example.com/kobo/tok123"
        );
    }

    #[test]
    fn endpoint_url_trims_a_trailing_slash_on_the_origin() {
        assert_eq!(
            endpoint_url("http://localhost:3000/", "abc"),
            "http://localhost:3000/kobo/abc"
        );
    }
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn card() -> Element {
        rsx! {
            KoboDevicesCard {}
        }
    }

    /// `Callback::new` needs a live runtime, so the row is built inside the
    /// throwaway `VirtualDom` rather than via a bare `render`.
    fn device_row() -> Element {
        rsx! {
            KoboDeviceRow {
                device: KoboDeviceView {
                    id: 1,
                    name: "Kobo Clara".to_string(),
                    token: "tok123".to_string(),
                    created_at: 0,
                    last_seen_at: 0,
                },
                endpoint: "https://omni.example.com/kobo/tok123".to_string(),
                disabled: false,
                on_regenerate: Callback::new(|_| {}),
                on_remove: Callback::new(|_| {}),
            }
        }
    }

    /// The warning sits at card level, above the list, so it is present on the
    /// very first paint — before any device row (and therefore any endpoint
    /// URL) can render. `KoboDeviceRow` is only ever mounted inside this card,
    /// so covering the card covers every path that shows a URL (#927 AC1/AC4).
    #[test]
    fn kobo_card_renders_the_data_loss_warning_on_first_paint() {
        let html = render_in_vdom(card);
        assert!(html.contains("data-testid=\"kobo-data-loss-warning\""));
        assert!(html.contains("First sync can erase your Kobo&#39;s highlights"));
        assert!(html.contains("KoboReader.sqlite"));
    }

    /// The setup steps must name the real procedure — the USB edit of
    /// `Kobo eReader.conf` — and never a nonexistent on-device setting
    /// (the pre-#1499 copy pointed at "Settings → Beta Features").
    #[test]
    fn kobo_card_walks_through_the_conf_file_setup() {
        let html = render_in_vdom(card);
        assert!(html.contains("data-testid=\"kobo-setup-steps\""));
        assert!(html.contains("Kobo eReader.conf"));
        assert!(html.contains("api_endpoint="));
        assert!(!html.contains("Beta Features"));
    }

    /// The warning must not be gated on having registered a device — a user
    /// reads it *before* adding their first Kobo, not after.
    #[test]
    fn kobo_card_warns_even_with_no_devices_registered() {
        let html = render_in_vdom(card);
        assert!(html.contains("data-testid=\"kobo-devices-empty\""));
        assert!(html.contains("data-testid=\"kobo-data-loss-warning\""));
    }

    /// The row is what actually exposes the endpoint URL; assert it renders so
    /// the card-level warning test above is anchored to a real URL surface.
    #[test]
    fn kobo_device_row_renders_the_endpoint_url() {
        let html = render_in_vdom(device_row);
        assert!(html.contains("https://omni.example.com/kobo/tok123"));
    }
}
