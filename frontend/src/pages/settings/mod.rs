//! Settings page (`/settings`) — library-path form + live file summaries.
//!
//! Admin-only. Hydrates the saved [`Settings`], lets the admin point Omnibus
//! at ebook / audiobook directories, and shows recursive per-extension
//! counts via the scanner so changes can be eyeballed before saving.

// Only compiled on web/server: its body calls the web-only
// `data::get_metadata_precedence`/`save_metadata_precedence` (no mobile RPC
// route yet).
#[cfg(not(feature = "mobile"))]
mod metadata_precedence;
mod smtp;

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{
    HardcoverKeyStatus, LibraryContents, LibrarySection, Settings, SCAN_INTERVAL_MIN_HOURS,
};

#[cfg(not(feature = "mobile"))]
use crate::components::worker_status::WorkerStatusIndicator;
use crate::{data, use_server_url, Route};

#[cfg(not(feature = "mobile"))]
use metadata_precedence::MetadataPrecedenceField;
use smtp::SmtpConfigField;

/// Library paths settings form + live recursive file-count summaries.
#[component]
pub fn SettingsPage() -> Element {
    let server_url = use_server_url();

    let ebook_path = use_signal(String::new);
    let audiobook_path = use_signal(String::new);
    let scan_interval_hours = use_signal(String::new);
    let status = use_signal(|| None::<String>);
    let status_is_error = use_signal(|| false);
    let library = use_signal(LibraryContents::default);
    let refetch_in_flight = use_signal(|| false);
    let backfill_in_flight = use_signal(|| false);
    let scan_in_flight = use_signal(|| false);
    // Bumped after a successful save to re-trigger the library-refresh effect.
    let library_refresh = use_signal(|| 0u32);

    // Admin gating for the F3.3 Hardcover key card — derived from the
    // app-wide `CurrentUser` context (`crate::use_is_admin`) instead of an
    // independent per-mount `/api/auth/me` fetch. The server-side `AdminUser`
    // extractor on the key RPCs is the real boundary; this just keeps the
    // card off non-admin screens.
    let is_admin = crate::use_is_admin();

    spawn_initial_settings_load(
        server_url.clone(),
        ebook_path,
        audiobook_path,
        scan_interval_hours,
        status,
        status_is_error,
    );
    spawn_library_refresh(server_url.clone(), library, library_refresh);

    let on_submit = save_settings_handler(
        server_url,
        ebook_path,
        audiobook_path,
        scan_interval_hours,
        status,
        status_is_error,
        library_refresh,
    );

    rsx! {
        div { class: "settings-page",
        section { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Configure your library paths." }

            form {
                id: "settings-form",
                class: "settings-form",
                onsubmit: on_submit,
                LibraryPathFields {
                    ebook_path,
                    audiobook_path,
                    library,
                }
                ScanIntervalField { scan_interval_hours }
                {worker_status_slot()}

                div { class: "settings-actions",
                    button { r#type: "submit", class: "btn", "Save" }
                    MaintenanceActions {
                        status,
                        status_is_error,
                        refetch_in_flight,
                        backfill_in_flight,
                        scan_in_flight,
                        library_refresh,
                    }
                }
            }

            p {
                id: "settings-status",
                "data-testid": "settings-status",
                role: "status",
                class: if status_is_error() { "settings-status error" } else { "settings-status success" },
                if let Some(msg) = status() { "{msg}" }
            }
        }
        if is_admin() {
            {metadata_precedence_slot()}
            HardcoverKeyField {}
            SmtpConfigField {}
            section { class: "card", "data-testid": "logs-link-card",
                h2 { "Server logs" }
                p { class: "subtitle",
                    "Browse the on-disk structured logs with level, module, and time-range filters."
                }
                Link { to: Route::Logs {}, class: "btn", "data-testid": "logs-link", "Open log viewer" }
            }
        }
        }
    }
}

/// Metadata-precedence card slot — emits [`MetadataPrecedenceField`] on
/// web/server, nothing on mobile (no mobile RPC route yet; see
/// `data::get_metadata_precedence`'s doc comment).
fn metadata_precedence_slot() -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        rsx! { MetadataPrecedenceField {} }
    }
    #[cfg(feature = "mobile")]
    {
        rsx! {}
    }
}

/// Worker-status indicator slot — emits `WorkerStatusIndicator` on web/server, nothing on mobile.
fn worker_status_slot() -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        rsx! { WorkerStatusIndicator {} }
    }
    #[cfg(feature = "mobile")]
    {
        rsx! {}
    }
}

/// Hydrates the saved [`Settings`] into the path inputs on mount.
fn spawn_initial_settings_load(
    url: String,
    mut ebook_path: Signal<String>,
    mut audiobook_path: Signal<String>,
    mut scan_interval_hours: Signal<String>,
    mut status: Signal<Option<String>>,
    mut status_is_error: Signal<bool>,
) {
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            match data::get_settings(&url).await {
                Ok(settings) => {
                    ebook_path.set(settings.ebook_library_path.unwrap_or_default());
                    audiobook_path.set(settings.audiobook_library_path.unwrap_or_default());
                    scan_interval_hours.set(
                        settings
                            .scan_interval_hours
                            .map(|h| h.to_string())
                            .unwrap_or_default(),
                    );
                }
                Err(e) => {
                    status.set(Some(e.to_string()));
                    status_is_error.set(true);
                }
            }
        });
    });
}

/// Refetches the live per-extension counts whenever `library_refresh` ticks.
fn spawn_library_refresh(
    url: String,
    mut library: Signal<LibraryContents>,
    library_refresh: Signal<u32>,
) {
    use_effect(move || {
        let _ = library_refresh();
        let url = url.clone();
        spawn(async move {
            if let Ok(contents) = data::get_library(&url).await {
                library.set(contents);
            }
        });
    });
}

/// Returns the `<form onsubmit>` handler that POSTs the path + interval inputs.
fn save_settings_handler(
    url: String,
    ebook_path: Signal<String>,
    audiobook_path: Signal<String>,
    scan_interval_hours: Signal<String>,
    mut status: Signal<Option<String>>,
    mut status_is_error: Signal<bool>,
    mut library_refresh: Signal<u32>,
) -> impl FnMut(FormEvent) {
    move |evt: FormEvent| {
        evt.prevent_default();
        let url = url.clone();
        let ebook = ebook_path().trim().to_string();
        let audiobook = audiobook_path().trim().to_string();
        let interval_input = scan_interval_hours().trim().to_string();
        // Parsed client-side so a non-numeric entry gets an immediate,
        // specific message instead of a generic save failure; an in-range
        // numeric value still round-trips through `Settings::validate()`
        // server-side (e.g. the `>= 1` floor).
        let interval = if interval_input.is_empty() {
            None
        } else {
            match interval_input.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    status.set(Some(
                        "Automatic rescan interval must be a whole number of hours.".to_string(),
                    ));
                    status_is_error.set(true);
                    return;
                }
            }
        };
        spawn(async move {
            let payload = Settings {
                ebook_library_path: (!ebook.is_empty()).then_some(ebook),
                audiobook_library_path: (!audiobook.is_empty()).then_some(audiobook),
                scan_interval_hours: interval,
            };
            match data::save_settings(&url, payload).await {
                Ok(_) => {
                    status.set(Some("Settings saved.".to_string()));
                    status_is_error.set(false);
                    library_refresh.set(library_refresh() + 1);
                }
                Err(_) => {
                    status.set(Some("Failed to save settings.".to_string()));
                    status_is_error.set(true);
                }
            }
        });
    }
}

/// Ebook + audiobook path inputs with their live per-extension summaries.
#[component]
fn LibraryPathFields(
    mut ebook_path: Signal<String>,
    mut audiobook_path: Signal<String>,
    library: Signal<LibraryContents>,
) -> Element {
    rsx! {
        div { class: "settings-field",
            label { r#for: "ebook-library-path", "Ebook Library Path" }
            input {
                r#type: "text",
                id: "ebook-library-path",
                name: "ebook_library_path",
                value: "{ebook_path}",
                placeholder: "/path/to/ebooks",
                oninput: move |evt| ebook_path.set(evt.value()),
            }
            LibrarySummary {
                testid: "ebook-library-summary",
                section: library().ebooks,
            }
        }
        div { class: "settings-field",
            label { r#for: "audiobook-library-path", "Audiobook Library Path" }
            input {
                r#type: "text",
                id: "audiobook-library-path",
                name: "audiobook_library_path",
                value: "{audiobook_path}",
                placeholder: "/path/to/audiobooks",
                oninput: move |evt| audiobook_path.set(evt.value()),
            }
            LibrarySummary {
                testid: "audiobook-library-summary",
                section: library().audiobooks,
            }
        }
    }
}

/// Optional periodic-rescan interval, in hours. Blank disables it — the
/// server-side `Settings::validate()` rejects 0 or any other value below
/// [`omnibus_shared::SCAN_INTERVAL_MIN_HOURS`]; a non-numeric entry is
/// caught client-side by [`save_settings_handler`] before the request fires.
#[component]
fn ScanIntervalField(mut scan_interval_hours: Signal<String>) -> Element {
    rsx! {
        div { class: "settings-field",
            label { r#for: "scan-interval-hours", "Automatic Rescan Interval (hours)" }
            input {
                r#type: "number",
                min: "{SCAN_INTERVAL_MIN_HOURS}",
                id: "scan-interval-hours",
                name: "scan_interval_hours",
                "data-testid": "scan-interval-hours",
                value: "{scan_interval_hours}",
                placeholder: "Leave blank to disable",
                oninput: move |evt| scan_interval_hours.set(evt.value()),
            }
        }
    }
}

/// Ghost buttons for one-off maintenance jobs (library rescan, author photo
/// refetch, chapter backfill).
#[component]
fn MaintenanceActions(
    mut status: Signal<Option<String>>,
    mut status_is_error: Signal<bool>,
    mut refetch_in_flight: Signal<bool>,
    mut backfill_in_flight: Signal<bool>,
    mut scan_in_flight: Signal<bool>,
    mut library_refresh: Signal<u32>,
) -> Element {
    let server_url = use_server_url();
    let url_for_scan = server_url.clone();
    let url_for_refetch = server_url.clone();
    let url_for_backfill = server_url;

    rsx! {
        button {
            r#type: "button",
            class: "btn ghost",
            disabled: scan_in_flight(),
            "data-testid": "scan-library",
            onclick: move |_| {
                let url = url_for_scan.clone();
                scan_in_flight.set(true);
                spawn(async move {
                    match data::scan_library(&url).await {
                        Ok(()) => {
                            status.set(Some("Library scan queued.".into()));
                            status_is_error.set(false);
                            library_refresh.set(library_refresh() + 1);
                        }
                        Err(e) => {
                            status.set(Some(format!("Failed to start library scan: {e}")));
                            status_is_error.set(true);
                        }
                    }
                    scan_in_flight.set(false);
                });
            },
            "Scan Library"
        }
        button {
            r#type: "button",
            class: "btn ghost",
            disabled: refetch_in_flight(),
            "data-testid": "refetch-author-photos",
            onclick: move |_| {
                let url = url_for_refetch.clone();
                refetch_in_flight.set(true);
                spawn(async move {
                    match data::refetch_author_photos(&url).await {
                        Ok(()) => {
                            status.set(Some("Author photo refetch queued.".into()));
                            status_is_error.set(false);
                            refetch_in_flight.set(false);
                        }
                        Err(e) => {
                            status.set(Some(format!("Failed to start photo refetch: {e}")));
                            status_is_error.set(true);
                            refetch_in_flight.set(false);
                        }
                    }
                });
            },
            "Refetch Author Pictures"
        }
        button {
            r#type: "button",
            class: "btn ghost",
            disabled: backfill_in_flight(),
            "data-testid": "backfill-chapters",
            onclick: move |_| {
                let url = url_for_backfill.clone();
                backfill_in_flight.set(true);
                spawn(async move {
                    match data::backfill_chapters(&url).await {
                        Ok(()) => {
                            status.set(Some("Chapter extraction queued.".into()));
                            status_is_error.set(false);
                            backfill_in_flight.set(false);
                        }
                        Err(e) => {
                            status.set(Some(format!("Failed to start chapter extraction: {e}")));
                            status_is_error.set(true);
                            backfill_in_flight.set(false);
                        }
                    }
                });
            },
            "Extract Audiobook Chapters"
        }
    }
}

/// Admin field to set/clear the server-wide Hardcover API key. Loads the
/// masked status on mount; the raw key is never read back to the client.
#[component]
fn HardcoverKeyField() -> Element {
    let server_url = use_server_url();
    let mut status: Signal<Option<HardcoverKeyStatus>> = use_signal(|| None);
    let mut key_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    let load_url = server_url.clone();
    use_effect(move || {
        let url = load_url.clone();
        spawn(async move {
            if let Ok(s) = data::get_hardcover_key(&url).await {
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
            msg.set(Some("Enter a Hardcover key to save.".to_string()));
            msg_is_error.set(true);
            return;
        }
        let url = save_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::set_hardcover_key(&url, Some(value)).await {
                Ok(s) => {
                    status.set(Some(s));
                    key_input.set(String::new());
                    msg.set(Some("Hardcover key saved.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to save Hardcover key.".to_string()));
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
            match data::set_hardcover_key(&url, None).await {
                Ok(s) => {
                    status.set(Some(s));
                    key_input.set(String::new());
                    msg.set(Some("Hardcover key cleared.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to clear Hardcover key.".to_string()));
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
        .unwrap_or_else(|| "hc_live_\u{2026}".to_string());

    rsx! {
        section { class: "card", "data-testid": "hardcover-key-card",
            h2 { "Suggestions" }
            p { class: "subtitle", "Connect Hardcover to power \u{201c}Readers also enjoyed\u{201d}." }
            div { class: "settings-field",
                label { r#for: "hardcover-key", "Hardcover API Key" }
                input {
                    r#type: "password",
                    id: "hardcover-key",
                    name: "hardcover_api_key",
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
                    "data-testid": "hardcover-save",
                    onclick: on_save,
                    "Save"
                }
                if configured {
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: in_flight(),
                        "data-testid": "hardcover-clear",
                        onclick: on_clear,
                        "Clear"
                    }
                }
            }
            div { class: "hardcover-status mono", "data-testid": "hardcover-status",
                if configured {
                    span { class: "hardcover-dot connected" }
                    if let Some(s) = st.as_ref() {
                        "Connected \u{00b7} {s.source} \u{00b7} {s.masked.clone().unwrap_or_default()}"
                    }
                } else {
                    span { class: "hardcover-dot" }
                    "Not connected"
                }
            }
            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "hardcover-key-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

fn library_summary_line(section: &LibrarySection) -> String {
    let mut line = format!("{} file(s) found.", section.total_files);
    for (ext, count) in &section.counts_by_ext {
        line.push_str(&format!(" {count} .{ext} found."));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(total: usize, counts: &[(&str, usize)]) -> LibrarySection {
        LibrarySection {
            path: Some("/lib".into()),
            total_files: total,
            counts_by_ext: counts.iter().map(|(e, c)| (e.to_string(), *c)).collect(),
            error: None,
        }
    }

    #[test]
    fn summary_line_reports_total_only_when_no_ext_breakdown() {
        assert_eq!(library_summary_line(&section(3, &[])), "3 file(s) found.");
    }

    #[test]
    fn summary_line_appends_a_clause_per_extension() {
        let line = library_summary_line(&section(5, &[("epub", 4), ("pdf", 1)]));
        assert!(line.starts_with("5 file(s) found."));
        assert!(line.contains("4 .epub found."));
        assert!(line.contains("1 .pdf found."));
    }
}

#[component]
fn LibrarySummary(testid: String, section: LibrarySection) -> Element {
    if section.path.is_none() {
        return rsx! {
            p {
                id: "{testid}",
                "data-testid": "{testid}",
                class: "library-summary empty",
            }
        };
    }

    if let Some(err) = &section.error {
        return rsx! {
            p {
                id: "{testid}",
                "data-testid": "{testid}",
                class: "library-summary error",
                "⚠ {err}"
            }
        };
    }

    let line = library_summary_line(&section);

    rsx! {
        p {
            id: "{testid}",
            "data-testid": "{testid}",
            class: "library-summary",
            "{line}"
        }
    }
}
