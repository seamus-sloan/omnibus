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

    let mut ebook_path = use_signal(String::new);
    let mut audiobook_path = use_signal(String::new);
    let mut status = use_signal(|| None::<String>);
    let mut status_is_error = use_signal(|| false);
    let mut library = use_signal(LibraryContents::default);
    // Bumped after a successful save to re-trigger the library-refresh effect.
    let mut library_refresh = use_signal(|| 0u32);

    let url_for_initial = server_url.clone();
    use_effect(move || {
        let url = url_for_initial.clone();
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

    let url_for_library = server_url.clone();
    use_effect(move || {
        let _ = library_refresh();
        let url = url_for_library.clone();
        spawn(async move {
            if let Ok(contents) = data::get_library(&url).await {
                library.set(contents);
            }
        });
    });

    let worker_status_slot: Element = {
        #[cfg(not(feature = "mobile"))]
        {
            rsx! { WorkerStatusIndicator {} }
        }
        #[cfg(feature = "mobile")]
        {
            rsx! {}
        }
    };

    rsx! {
        section { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Configure your library paths." }

            form {
                id: "settings-form",
                class: "settings-form",
                onsubmit: move |evt| {
                    evt.prevent_default();
                    let url = server_url.clone();
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
                },
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
                // Background-worker progress (scan / thumbs / author
                // photos / future cleanup actions). Mounted above the Save
                // button so the user sees the post-save scan kick in
                // without leaving the page. Web-only; the mobile build
                // omits the indicator entirely until issue #69 follow-up
                // ships a REST mirror. `cfg` attrs aren't legal directly
                // on rsx component calls, so the slot is bound as an
                // Element outside the macro and embedded by reference.
                {worker_status_slot}

                button { r#type: "submit", class: "btn", "Save" }
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
