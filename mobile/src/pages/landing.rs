use dioxus::prelude::*;

use crate::ServerUrl;

#[component]
pub fn LandingPage() -> Element {
    let server_url = use_context::<ServerUrl>();
    let server_url_for_effect = server_url.clone();
    let mut value = use_signal(|| 0i64);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        let url = server_url_for_effect.0.clone();
        spawn(async move {
            match fetch_value(&url).await {
                Ok(v) => { value.set(v); error.set(None); }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    rsx! {
        div { class: "card",
            h1 { "Omnibus Counter" }
            p { class: "subtitle", "Dioxus Native + Rust backend + SQLite" }
            if let Some(msg) = error() {
                p { class: "error", "⚠ {msg}" }
            }
            p { class: "value-line", "Current value: {value}" }
            button {
                class: "btn",
                onclick: move |_| {
                    let url = server_url.0.clone();
                    spawn(async move {
                        match post_increment(&url).await {
                            Ok(v) => { value.set(v); error.set(None); }
                            Err(e) => error.set(Some(e)),
                        }
                    });
                },
                "Increment value"
            }
        }
    }
}

async fn fetch_value(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value");
    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"].as_i64().ok_or_else(|| "missing value field".into())
}

async fn post_increment(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value/increment");
    let client = reqwest::Client::new();
    let response = client.post(&url).send().await.map_err(|e| e.to_string())?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"].as_i64().ok_or_else(|| "missing value field".into())
}
