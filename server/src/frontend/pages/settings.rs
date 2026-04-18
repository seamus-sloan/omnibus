use dioxus::prelude::*;

use crate::db::Settings;

#[component]
pub fn SettingsPage(settings: Option<Settings>) -> Element {
    let settings = settings.unwrap_or_default();
    let ebook_value = settings.ebook_library_path.unwrap_or_default();
    let audiobook_value = settings.audiobook_library_path.unwrap_or_default();

    rsx! {
        section { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Configure your library paths on the server." }

            form { id: "settings-form", class: "settings-form",
                div { class: "settings-field",
                    label { r#for: "ebook-library-path", "Ebook Library Path" }
                    input {
                        r#type: "text",
                        id: "ebook-library-path",
                        name: "ebook_library_path",
                        value: "{ebook_value}",
                        placeholder: "/path/to/ebooks"
                    }
                }
                div { class: "settings-field",
                    label { r#for: "audiobook-library-path", "Audiobook Library Path" }
                    input {
                        r#type: "text",
                        id: "audiobook-library-path",
                        name: "audiobook_library_path",
                        value: "{audiobook_value}",
                        placeholder: "/path/to/audiobooks"
                    }
                }
                button { r#type: "submit", class: "btn", "Save" }
            }

            p { id: "settings-status", role: "status", class: "settings-status" }
        }

        section { class: "card library-card",
            h2 { "Ebook Library" }
            div { id: "ebook-library-contents", "data-testid": "ebook-library-contents", class: "library-contents",
                p { class: "library-loading", "Loading…" }
            }
        }

        section { class: "card library-card",
            h2 { "Audiobook Library" }
            div { id: "audiobook-library-contents", "data-testid": "audiobook-library-contents", class: "library-contents",
                p { class: "library-loading", "Loading…" }
            }
        }
    }
}
