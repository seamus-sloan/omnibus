use dioxus::prelude::*;
use omnibus_shared::{LibraryContents, LibrarySection};

use crate::ServerUrl;

#[component]
pub fn SettingsPage() -> Element {
    let server_url = use_context::<ServerUrl>();
    let server_url_for_settings = server_url.clone();
    let server_url_for_library = server_url.clone();

    let mut ebook_path = use_signal(String::new);
    let mut audiobook_path = use_signal(String::new);
    let mut status = use_signal(|| None::<String>);
    let mut status_is_error = use_signal(|| false);
    let mut library = use_signal(LibraryContents::default);
    // Incrementing this triggers a library refresh.
    let mut library_refresh = use_signal(|| 0u32);

    use_effect(move || {
        let url = server_url_for_settings.0.clone();
        spawn(async move {
            match fetch_settings(&url).await {
                Ok((ebook, audiobook)) => {
                    ebook_path.set(ebook);
                    audiobook_path.set(audiobook);
                }
                Err(e) => {
                    status.set(Some(e));
                    status_is_error.set(true);
                }
            }
        });
    });

    use_effect(move || {
        let _ = library_refresh();
        let url = server_url_for_library.0.clone();
        spawn(async move {
            if let Ok(contents) = fetch_library(&url).await {
                library.set(contents);
            }
        });
    });

    rsx! {
        div { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Connected to: {server_url.0}" }

            div { class: "settings-form",
                div { class: "settings-field",
                    label { class: "settings-label", "Ebook Library Path" }
                    input {
                        class: "settings-input",
                        value: "{ebook_path}",
                        placeholder: "/path/to/ebooks",
                        oninput: move |evt| ebook_path.set(evt.value()),
                    }
                }
                div { class: "settings-field",
                    label { class: "settings-label", "Audiobook Library Path" }
                    input {
                        class: "settings-input",
                        value: "{audiobook_path}",
                        placeholder: "/path/to/audiobooks",
                        oninput: move |evt| audiobook_path.set(evt.value()),
                    }
                }
                button {
                    class: "btn",
                    onclick: move |_| {
                        let url = server_url.0.clone();
                        let ebook = ebook_path().trim().to_string();
                        let audiobook = audiobook_path().trim().to_string();
                        spawn(async move {
                            match save_settings(
                                &url,
                                if ebook.is_empty() { None } else { Some(ebook) },
                                if audiobook.is_empty() { None } else { Some(audiobook) },
                            )
                            .await
                            {
                                Ok(()) => {
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
                    "Save"
                }
            }

            if let Some(msg) = status() {
                p {
                    class: if status_is_error() { "error" } else { "success-msg" },
                    "{msg}"
                }
            }
        }

        LibrarySectionView {
            title: "Ebook Library",
            section: library().ebooks,
        }
        LibrarySectionView {
            title: "Audiobook Library",
            section: library().audiobooks,
        }
    }
}

#[component]
fn LibrarySectionView(title: String, section: LibrarySection) -> Element {
    rsx! {
        div { class: "card library-card",
            h2 { class: "library-title", "{title}" }

            if section.path.is_none() {
                p { class: "library-empty", "No path configured." }
            } else if let Some(err) = &section.error {
                p { class: "error", "⚠ {err}" }
            } else if section.files.is_empty() {
                p { class: "library-empty",
                    "No files found in "
                    span { class: "library-path", "{section.path.as_deref().unwrap_or_default()}" }
                }
            } else {
                p { class: "library-path", "{section.path.as_deref().unwrap_or_default()}" }
                p { class: "library-count", "{section.files.len()} file(s)" }
                div { class: "library-file-list",
                    for file in &section.files {
                        p { class: "library-file", "{file}" }
                    }
                }
            }
        }
    }
}

async fn fetch_settings(server_url: &str) -> Result<(String, String), String> {
    let url = format!("{server_url}/api/settings");
    eprintln!("[mobile] GET {url}");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    let ebook = payload["ebook_library_path"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let audiobook = payload["audiobook_library_path"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok((ebook, audiobook))
}

async fn fetch_library(server_url: &str) -> Result<LibraryContents, String> {
    let url = format!("{server_url}/api/library");
    eprintln!("[mobile] GET {url}");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    Ok(LibraryContents {
        ebooks: parse_section(&payload["ebooks"]),
        audiobooks: parse_section(&payload["audiobooks"]),
    })
}

fn parse_section(v: &serde_json::Value) -> LibrarySection {
    LibrarySection {
        path: v["path"].as_str().map(str::to_string),
        files: v["files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        error: v["error"].as_str().map(str::to_string),
    }
}

async fn save_settings(
    server_url: &str,
    ebook_library_path: Option<String>,
    audiobook_library_path: Option<String>,
) -> Result<(), String> {
    let url = format!("{server_url}/api/settings");
    eprintln!("[mobile] POST {url}");
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "ebook_library_path": ebook_library_path,
        "audiobook_library_path": audiobook_library_path,
    });
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Server error: {}", response.status()))
    }
}
