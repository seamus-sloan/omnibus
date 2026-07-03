//! Add-books page (`/add-books`) — upload an EPUB into the library.
//!
//! Upload-permitted users pick an EPUB; the server parses it and returns its
//! metadata for an editable confirm step, then files the file into the
//! canonical library folder and redirects to the new book. Audiobook upload is
//! present but stubbed ("coming soon"). The rsx is identical on every target —
//! file interop only runs inside the post-mount `spawn`, preserving hydration
//! parity (see rule 07).

use dioxus::prelude::*;
use dioxus_router::use_navigator;

use crate::data::{self, EbookUploadMeta};
use crate::{use_server_url, Route};

/// Upload form: pick an EPUB, confirm/correct the auto-extracted metadata, file it.
#[component]
pub fn AddBooksPage() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();

    let mut filename = use_signal(String::new);
    let mut file_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
    let mut title = use_signal(String::new);
    let mut author = use_signal(String::new);
    let mut series = use_signal(String::new);
    let mut series_index = use_signal(String::new);
    let mut inspected = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut status: Signal<Option<String>> = use_signal(|| None);
    let mut status_is_error = use_signal(|| false);

    // File select → read bytes → inspect → pre-fill the editable fields.
    let on_file = {
        let server_url = server_url.clone();
        move |evt: Event<FormData>| {
            let Some(file) = evt.files().into_iter().next() else {
                return;
            };
            let name = file.name();
            let server_url = server_url.clone();
            busy.set(true);
            status.set(Some(format!("Reading {name}\u{2026}")));
            status_is_error.set(false);
            spawn(async move {
                match file.read_bytes().await {
                    Ok(bytes) => {
                        let bytes = bytes.to_vec();
                        match data::inspect_ebook(&server_url, name.clone(), bytes.clone()).await {
                            Ok(insp) => {
                                title.set(insp.title.unwrap_or_default());
                                author.set(insp.author.unwrap_or_default());
                                series.set(insp.series.unwrap_or_default());
                                series_index.set(insp.series_index.unwrap_or_default());
                                filename.set(name);
                                file_bytes.set(Some(bytes));
                                inspected.set(true);
                                status.set(Some(
                                    "Review the details, then add to your library.".into(),
                                ));
                                status_is_error.set(false);
                            }
                            Err(e) => {
                                status.set(Some(format!("Could not read that EPUB: {e}")));
                                status_is_error.set(true);
                            }
                        }
                    }
                    Err(e) => {
                        status.set(Some(format!("Could not read that file: {e}")));
                        status_is_error.set(true);
                    }
                }
                busy.set(false);
            });
        }
    };

    // Confirm → file the book → redirect to its detail page.
    let on_submit = {
        let server_url = server_url.clone();
        move |evt: FormEvent| {
            evt.prevent_default();
            let server_url = server_url.clone();
            let confirmed_title = title().trim().to_string();
            let confirmed_author = author().trim().to_string();
            if confirmed_title.is_empty() || confirmed_author.is_empty() {
                status.set(Some("Title and author are required.".into()));
                status_is_error.set(true);
                return;
            }
            let Some(bytes) = file_bytes() else {
                status.set(Some("Choose an EPUB file first.".into()));
                status_is_error.set(true);
                return;
            };
            let name = filename();
            let meta = EbookUploadMeta {
                title: confirmed_title,
                author: confirmed_author,
                series: series().trim().to_string(),
                series_index: series_index().trim().to_string(),
            };
            busy.set(true);
            status.set(Some("Adding to your library\u{2026}".into()));
            status_is_error.set(false);
            spawn(async move {
                match data::upload_ebook(&server_url, name, bytes, meta).await {
                    Ok(result) => {
                        nav.push(Route::BookDetail { uuid: result.uuid });
                    }
                    Err(e) => {
                        status.set(Some(format!("Upload failed: {e}")));
                        status_is_error.set(true);
                        busy.set(false);
                    }
                }
            });
        }
    };

    rsx! {
        section { class: "card",
            h1 { "Add books" }
            p { class: "subtitle",
                "Upload an EPUB and Omnibus will file it into your library."
            }

            // Type toggle — ebook active, audiobook stubbed until audio ingest lands.
            div {
                class: "add-books-types",
                role: "group",
                aria_label: "Upload type",
                button {
                    r#type: "button",
                    class: "btn",
                    aria_pressed: "true",
                    "data-testid": "add-books-type-ebook",
                    "Ebook"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: true,
                    aria_disabled: "true",
                    title: "Audiobook upload is coming soon",
                    "data-testid": "add-books-type-audiobook",
                    "Audiobook (coming soon)"
                }
            }

            div { class: "settings-field",
                span { class: "settings-label", "EPUB file" }
                div {
                    class: if filename().is_empty() { "file-drop-zone" } else { "file-drop-zone has-file" },
                    div { class: "file-drop-content",
                        if filename().is_empty() {
                            svg {
                                class: "file-drop-icon",
                                width: "28", height: "28",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "1.5",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
                                polyline { points: "17 8 12 3 7 8" }
                                line { x1: "12", y1: "3", x2: "12", y2: "15" }
                            }
                            span { class: "file-drop-prompt",
                                "Drop an EPUB here or "
                                strong { "choose a file" }
                            }
                        } else {
                            svg {
                                class: "file-drop-icon file-drop-icon--ok",
                                width: "28", height: "28",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "1.5",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                polyline { points: "20 6 9 17 4 12" }
                            }
                            span { class: "file-drop-filename", "{filename()}" }
                            span { class: "file-drop-change", "Click to change" }
                        }
                    }
                    input {
                        id: "add-books-file",
                        r#type: "file",
                        accept: ".epub,application/epub+zip",
                        "data-testid": "add-books-file-input",
                        aria_label: "EPUB file",
                        class: "file-drop-input",
                        disabled: busy(),
                        onchange: on_file,
                    }
                }
            }

            if inspected() {
                form {
                    id: "add-books-form",
                    class: "settings-form",
                    onsubmit: on_submit,

                    div { class: "settings-field",
                        label { r#for: "add-books-title", "Title" }
                        input {
                            id: "add-books-title",
                            r#type: "text",
                            value: "{title}",
                            disabled: busy(),
                            oninput: move |e| title.set(e.value()),
                        }
                    }
                    div { class: "settings-field",
                        label { r#for: "add-books-author", "Author" }
                        input {
                            id: "add-books-author",
                            r#type: "text",
                            value: "{author}",
                            disabled: busy(),
                            oninput: move |e| author.set(e.value()),
                        }
                    }
                    div { class: "settings-field",
                        label { r#for: "add-books-series", "Series" }
                        input {
                            id: "add-books-series",
                            r#type: "text",
                            value: "{series}",
                            disabled: busy(),
                            oninput: move |e| series.set(e.value()),
                        }
                    }
                    div { class: "settings-field",
                        label { r#for: "add-books-series-index", "Series index" }
                        input {
                            id: "add-books-series-index",
                            r#type: "text",
                            value: "{series_index}",
                            disabled: busy(),
                            oninput: move |e| series_index.set(e.value()),
                        }
                    }

                    div { class: "settings-actions",
                        button {
                            r#type: "submit",
                            class: "btn",
                            disabled: busy(),
                            "data-testid": "add-books-submit",
                            "Add to library"
                        }
                    }
                }
            }

            if let Some(msg) = status() {
                p {
                    id: "add-books-status",
                    "data-testid": "add-books-status",
                    role: "status",
                    class: if status_is_error() { "settings-status error" } else { "settings-status success" },
                    "{msg}"
                }
            }
        }
    }
}
