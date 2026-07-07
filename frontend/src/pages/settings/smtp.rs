//! Admin-only SMTP relay config card for the settings page.
//!
//! Backs Send-to-Kindle: loads the masked config on mount, saves/clears the
//! stored secret, and fires a test email. The password is never read back to
//! the client; a blank password on Save preserves the stored secret.

use dioxus::prelude::*;

use omnibus_shared::{SmtpConfigStatus, SmtpConfigUpdate, SmtpSecurity};

use crate::{data, use_server_url};

/// Admin field to configure the server-wide SMTP relay (F4.3) used by
/// Send-to-Kindle. Loads the masked status on mount; the password is never read
/// back to the client. A blank password on Save preserves the stored secret.
#[component]
pub fn SmtpConfigField() -> Element {
    let server_url = use_server_url();
    let fields = SmtpFields {
        host: use_signal(String::new),
        port: use_signal(|| "587".to_string()),
        username: use_signal(String::new),
        from_email: use_signal(String::new),
        password: use_signal(String::new),
        security: use_signal(|| SmtpSecurity::Starttls),
    };
    let io = SmtpIo {
        status: use_signal(|| None),
        msg: use_signal(|| None),
        msg_is_error: use_signal(|| false),
        in_flight: use_signal(|| false),
    };

    spawn_smtp_config_load(server_url.clone(), fields, io.status);

    let on_save = make_smtp_save(server_url.clone(), fields, io);
    let on_clear = make_smtp_clear(server_url.clone(), fields, io);
    let on_test = make_smtp_test(server_url, io);

    let st = (io.status)();
    let configured = st.as_ref().map(|s| s.configured).unwrap_or(false);

    rsx! {
        section { class: "card", "data-testid": "smtp-card",
            h2 { "Email delivery (SMTP)" }
            p { class: "subtitle", "Configure the relay used by Send-to-Kindle." }
            SmtpConnectionFields { fields, configured }
            SmtpTestActions {
                configured,
                in_flight: io.in_flight,
                status: st,
                msg: io.msg,
                msg_is_error: io.msg_is_error,
                on_save: EventHandler::new(on_save),
                on_test: EventHandler::new(on_test),
                on_clear: EventHandler::new(on_clear),
            }
        }
    }
}

/// Editable SMTP connection signals threaded into the connection fields.
#[derive(Copy, Clone, PartialEq)]
struct SmtpFields {
    host: Signal<String>,
    port: Signal<String>,
    username: Signal<String>,
    from_email: Signal<String>,
    password: Signal<String>,
    security: Signal<SmtpSecurity>,
}

/// Loaded status + save/test message + in-flight signals for the SMTP card.
#[derive(Copy, Clone, PartialEq)]
struct SmtpIo {
    status: Signal<Option<SmtpConfigStatus>>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    in_flight: Signal<bool>,
}

/// Load the masked SMTP config on mount and seed the editable fields.
fn spawn_smtp_config_load(
    server_url: String,
    fields: SmtpFields,
    mut status: Signal<Option<SmtpConfigStatus>>,
) {
    let mut host = fields.host;
    let mut port = fields.port;
    let mut username = fields.username;
    let mut from_email = fields.from_email;
    let mut security = fields.security;
    use_effect(move || {
        let url = server_url.clone();
        spawn(async move {
            if let Ok(s) = data::get_smtp_config(&url).await {
                if let Some(h) = s.host.clone() {
                    host.set(h);
                }
                if let Some(p) = s.port {
                    port.set(p.to_string());
                }
                if let Some(u) = s.username.clone() {
                    username.set(u);
                }
                if let Some(f) = s.from_email.clone() {
                    from_email.set(f);
                }
                security.set(s.security);
                status.set(Some(s));
            }
        });
    });
}

/// Build the Save handler: validate the port, then POST the config update.
fn make_smtp_save(server_url: String, fields: SmtpFields, io: SmtpIo) -> impl FnMut(MouseEvent) {
    let host = fields.host;
    let port = fields.port;
    let username = fields.username;
    let from_email = fields.from_email;
    let mut password = fields.password;
    let security = fields.security;
    let mut status = io.status;
    let mut msg = io.msg;
    let mut msg_is_error = io.msg_is_error;
    let mut in_flight = io.in_flight;
    move |_| {
        let host_val = host().trim().to_string();
        // `parse::<u16>()` accepts 0, but 0 is not a usable port — reject it
        // so the UI matches the "1 and 65535" message (and the DB validator).
        let port_val = match port().trim().parse::<u16>() {
            Ok(p) if p != 0 => p,
            _ => {
                msg.set(Some(
                    "Port must be a number between 1 and 65535.".to_string(),
                ));
                msg_is_error.set(true);
                return;
            }
        };
        // A blank password field means "leave the stored secret unchanged".
        let pw = password();
        let password_val = if pw.is_empty() { None } else { Some(pw) };
        let update = SmtpConfigUpdate {
            host: host_val,
            port: port_val,
            username: username().trim().to_string(),
            from_email: from_email().trim().to_string(),
            security: security(),
            password: password_val,
        };
        let url = server_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::set_smtp_config(&url, update).await {
                Ok(s) => {
                    status.set(Some(s));
                    password.set(String::new());
                    msg.set(Some("SMTP settings saved.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to save SMTP settings.".to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    }
}

/// Build the Clear handler: wipe the stored config and reset the fields.
fn make_smtp_clear(server_url: String, fields: SmtpFields, io: SmtpIo) -> impl FnMut(MouseEvent) {
    let mut host = fields.host;
    let mut port = fields.port;
    let mut username = fields.username;
    let mut from_email = fields.from_email;
    let mut password = fields.password;
    let mut security = fields.security;
    let mut status = io.status;
    let mut msg = io.msg;
    let mut msg_is_error = io.msg_is_error;
    let mut in_flight = io.in_flight;
    move |_| {
        let url = server_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::clear_smtp_config(&url).await {
                Ok(s) => {
                    host.set(String::new());
                    port.set("587".to_string());
                    username.set(String::new());
                    from_email.set(String::new());
                    password.set(String::new());
                    security.set(SmtpSecurity::Starttls);
                    status.set(Some(s));
                    msg.set(Some("SMTP settings cleared.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to clear SMTP settings.".to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    }
}

/// Build the Send-Test handler: fire a test email via the configured relay.
fn make_smtp_test(server_url: String, io: SmtpIo) -> impl FnMut(MouseEvent) {
    let mut msg = io.msg;
    let mut msg_is_error = io.msg_is_error;
    let mut in_flight = io.in_flight;
    move |_| {
        let url = server_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::send_smtp_test(&url).await {
                Ok(()) => {
                    msg.set(Some("Test email sent to your Kindle address.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some(
                        "Test email failed — check the SMTP settings and your account's Kindle email."
                            .to_string(),
                    ));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    }
}

/// Host / port / security / username / password / from-email inputs.
#[component]
fn SmtpConnectionFields(fields: SmtpFields, configured: bool) -> Element {
    let mut host = fields.host;
    let mut port = fields.port;
    let mut username = fields.username;
    let mut from_email = fields.from_email;
    let mut password = fields.password;
    let mut security = fields.security;
    rsx! {
        div { class: "settings-field",
            label { r#for: "smtp-host", "SMTP Host" }
            input {
                r#type: "text",
                id: "smtp-host",
                name: "smtp_host",
                autocomplete: "off",
                placeholder: "smtp.example.com",
                value: "{host}",
                oninput: move |e| host.set(e.value()),
            }
        }
        div { class: "settings-field",
            label { r#for: "smtp-port", "Port" }
            input {
                r#type: "number",
                id: "smtp-port",
                name: "smtp_port",
                value: "{port}",
                oninput: move |e| port.set(e.value()),
            }
        }
        div { class: "settings-field",
            label { r#for: "smtp-security", "Security" }
            select {
                id: "smtp-security",
                name: "smtp_security",
                "data-testid": "smtp-security",
                value: "{security().as_str()}",
                onchange: move |e| security.set(SmtpSecurity::from_str_or_default(&e.value())),
                option { value: "starttls", "STARTTLS (587)" }
                option { value: "tls", "TLS (465)" }
            }
        }
        div { class: "settings-field",
            label { r#for: "smtp-username", "Username" }
            input {
                r#type: "text",
                id: "smtp-username",
                name: "smtp_username",
                autocomplete: "off",
                value: "{username}",
                oninput: move |e| username.set(e.value()),
            }
        }
        div { class: "settings-field",
            label { r#for: "smtp-password", "Password" }
            input {
                r#type: "password",
                id: "smtp-password",
                name: "smtp_password",
                autocomplete: "off",
                autocapitalize: "none",
                autocorrect: "off",
                spellcheck: "false",
                placeholder: if configured { "\u{2022}\u{2022}\u{2022}\u{2022} (unchanged)" } else { "" },
                value: "{password}",
                oninput: move |e| password.set(e.value()),
            }
        }
        div { class: "settings-field",
            label { r#for: "smtp-from", "From Email" }
            input {
                r#type: "email",
                id: "smtp-from",
                name: "smtp_from_email",
                autocomplete: "off",
                placeholder: "library@example.com",
                value: "{from_email}",
                oninput: move |e| from_email.set(e.value()),
            }
        }
    }
}

/// Save / send-test / clear buttons, the configured-state line, and the
/// save/test status message.
#[component]
fn SmtpTestActions(
    configured: bool,
    in_flight: Signal<bool>,
    status: Option<SmtpConfigStatus>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    on_save: EventHandler<MouseEvent>,
    on_test: EventHandler<MouseEvent>,
    on_clear: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "settings-actions",
            button {
                r#type: "button",
                class: "btn",
                disabled: in_flight(),
                "data-testid": "smtp-save",
                onclick: move |e| on_save.call(e),
                "Save"
            }
            if configured {
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: in_flight(),
                    "data-testid": "smtp-test",
                    onclick: move |e| on_test.call(e),
                    "Send Test"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: in_flight(),
                    "data-testid": "smtp-clear",
                    onclick: move |e| on_clear.call(e),
                    "Clear"
                }
            }
        }
        div { class: "hardcover-status mono", "data-testid": "smtp-status",
            if configured {
                span { class: "hardcover-dot connected" }
                if let Some(s) = status.as_ref() {
                    "Configured \u{00b7} {s.source}"
                }
            } else {
                span { class: "hardcover-dot" }
                "Not configured"
            }
        }
        if let Some(m) = msg() {
            p {
                role: "status",
                "data-testid": "smtp-config-status",
                class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                "{m}"
            }
        }
    }
}
