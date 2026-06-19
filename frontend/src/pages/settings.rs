//! Settings page (`/settings`) — library-path form + live file summaries.
//!
//! Admin-only. Hydrates the saved [`Settings`], lets the admin point Omnibus
//! at ebook / audiobook directories, and shows recursive per-extension
//! counts via the scanner so changes can be eyeballed before saving.

use dioxus::prelude::*;
use omnibus_shared::{LibraryContents, LibrarySection, Settings};

#[cfg(not(feature = "mobile"))]
use crate::components::worker_status::WorkerStatusIndicator;
use crate::{data, use_server_url};

/// Library paths settings form + live recursive file-count summaries.
#[component]
pub fn SettingsPage() -> Element {
    let server_url = use_server_url();

    let ebook_path = use_signal(String::new);
    let audiobook_path = use_signal(String::new);
    let status = use_signal(|| None::<String>);
    let status_is_error = use_signal(|| false);
    let library = use_signal(LibraryContents::default);
    let refetch_in_flight = use_signal(|| false);
    let backfill_in_flight = use_signal(|| false);
    // Bumped after a successful save to re-trigger the library-refresh effect.
    let library_refresh = use_signal(|| 0u32);

    spawn_initial_settings_load(
        server_url.clone(),
        ebook_path,
        audiobook_path,
        status,
        status_is_error,
    );
    spawn_library_refresh(server_url.clone(), library, library_refresh);

    let on_submit = save_settings_handler(
        server_url,
        ebook_path,
        audiobook_path,
        status,
        status_is_error,
        library_refresh,
    );

    rsx! {
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
                {worker_status_slot()}

                div { class: "settings-actions",
                    button { r#type: "submit", class: "btn", "Save" }
                    MaintenanceActions {
                        status,
                        status_is_error,
                        refetch_in_flight,
                        backfill_in_flight,
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
    }
}

/// Background-worker progress indicator. Mounted above the Save button so the
/// user sees the post-save scan kick in without leaving the page. Web/server
/// only; the mobile build omits the indicator entirely until the REST mirror
/// in issue #69 ships. `cfg` attrs aren't legal directly on rsx component
/// calls, so the slot is bound as an Element outside the macro and embedded
/// by reference.
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

/// Builds the `<form onsubmit>` handler that POSTs the path inputs and
/// reports success / failure via the status signal.
fn save_settings_handler(
    url: String,
    ebook_path: Signal<String>,
    audiobook_path: Signal<String>,
    mut status: Signal<Option<String>>,
    mut status_is_error: Signal<bool>,
    mut library_refresh: Signal<u32>,
) -> impl FnMut(FormEvent) {
    move |evt: FormEvent| {
        evt.prevent_default();
        let url = url.clone();
        let ebook = ebook_path().trim().to_string();
        let audiobook = audiobook_path().trim().to_string();
        spawn(async move {
            let payload = Settings {
                ebook_library_path: (!ebook.is_empty()).then_some(ebook),
                audiobook_library_path: (!audiobook.is_empty()).then_some(audiobook),
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

/// Ghost buttons for one-off maintenance jobs (author photo refetch, chapter backfill).
#[component]
fn MaintenanceActions(
    mut status: Signal<Option<String>>,
    mut status_is_error: Signal<bool>,
    mut refetch_in_flight: Signal<bool>,
    mut backfill_in_flight: Signal<bool>,
) -> Element {
    let server_url = use_server_url();
    let url_for_refetch = server_url.clone();
    let url_for_backfill = server_url;

    rsx! {
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
