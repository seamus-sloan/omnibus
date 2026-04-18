use dioxus::prelude::*;
use omnibus_shared::{LibraryContents, LibrarySection, Settings};

use crate::{data, use_server_url};

/// Library paths settings form + live directory listings.
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
                    status.set(Some(e));
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
                            Err(e) => {
                                status.set(Some(e));
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
                }
                button { r#type: "submit", class: "btn", "Save" }
            }

            p {
                id: "settings-status",
                role: "status",
                class: if status_is_error() { "settings-status error" } else { "settings-status success" },
                if let Some(msg) = status() { "{msg}" }
            }
        }

        LibrarySectionView {
            title: "Ebook Library",
            testid: "ebook-library-contents",
            section: library().ebooks,
        }
        LibrarySectionView {
            title: "Audiobook Library",
            testid: "audiobook-library-contents",
            section: library().audiobooks,
        }
    }
}

#[component]
fn LibrarySectionView(title: String, testid: String, section: LibrarySection) -> Element {
    rsx! {
        section { class: "card library-card",
            h2 { "{title}" }
            div {
                id: "{testid}",
                "data-testid": "{testid}",
                class: "library-contents",
                if section.path.is_none() {
                    p { class: "library-empty", "No path configured." }
                } else if let Some(err) = &section.error {
                    p { class: "library-error", "⚠ {err}" }
                } else if section.files.is_empty() {
                    p { class: "library-empty",
                        "No files found in "
                        code { "{section.path.as_deref().unwrap_or_default()}" }
                    }
                } else {
                    p { class: "library-path", "{section.path.as_deref().unwrap_or_default()}" }
                    p { class: "library-count", "{section.files.len()} file(s)" }
                    ul { class: "library-file-list",
                        for file in &section.files {
                            li { "{file}" }
                        }
                    }
                }
            }
        }
    }
}
