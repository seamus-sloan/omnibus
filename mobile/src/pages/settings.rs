use dioxus::prelude::*;

use crate::ServerUrl;

#[component]
pub fn SettingsPage() -> Element {
    let server_url = use_context::<ServerUrl>();
    let server_url_for_effect = server_url.clone();

    let mut ebook_path = use_signal(String::new);
    let mut audiobook_path = use_signal(String::new);
    let mut status = use_signal(|| None::<String>);
    let mut status_is_error = use_signal(|| false);

    use_effect(move || {
        let url = server_url_for_effect.0.clone();
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
    }
}

async fn fetch_settings(server_url: &str) -> Result<(String, String), String> {
    let url = format!("{server_url}/api/settings");
    eprintln!("[mobile] GET {url}");
    let response = reqwest::get(&url).await.map_err(|e| {
        let msg = format!("{e:#}");
        eprintln!("[mobile] GET {url} failed: {msg}");
        msg
    })?;
    eprintln!("[mobile] GET {url} -> {}", response.status());
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
        .map_err(|e| {
            let msg = format!("{e:#}");
            eprintln!("[mobile] POST {url} failed: {msg}");
            msg
        })?;
    eprintln!("[mobile] POST {url} -> {}", response.status());
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Server error: {}", response.status()))
    }
}
