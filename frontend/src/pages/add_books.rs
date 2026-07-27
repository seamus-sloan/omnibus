//! Add-books page (`/add-books`) — upload an EPUB or audiobook into the library.
//! Users pick a file; the server parses it for an editable confirm step, then
//! files it into the canonical folder and redirects to the new book. Ebooks
//! take one `.epub`; audiobooks one `.m4a`/`.m4b` or a set of `.mp3` parts. The
//! rsx is target-agnostic — file interop runs only in the post-mount `spawn`.
//!
//! Gated on `can_upload`: the real boundary is the server's `require_upload`
//! (`server/src/backend/uploads.rs`); this just keeps the form off a screen
//! the submit would 403 on, mirroring `pages::logs`'s `use_is_admin` gate.

use dioxus::prelude::*;
use dioxus_router::use_navigator;

use crate::data::{self, AudiobookUploadMeta, EbookUploadMeta};
use crate::{use_server_url, Route};

/// Which library the upload targets. Drives the file-picker `accept` list, the
/// confirm form's fields, and which data-layer call the submit handler makes.
#[derive(Copy, Clone, PartialEq, Eq)]
enum UploadMode {
    Ebook,
    Audiobook,
}

/// The editable-metadata + staged-upload signals threaded through the page's
/// handlers and confirm form. `Copy` so the async handlers can capture them.
#[derive(Copy, Clone, PartialEq)]
struct UploadState {
    mode: Signal<UploadMode>,
    filename: Signal<String>,
    /// Staged EPUB bytes (ebook mode).
    file_bytes: Signal<Option<Vec<u8>>>,
    /// Staged audiobook part(s) as `(filename, bytes)` (audiobook mode).
    audio_files: Signal<Vec<(String, Vec<u8>)>>,
    title: Signal<String>,
    author: Signal<String>,
    series: Signal<String>,
    series_index: Signal<String>,
    inspected: Signal<bool>,
    busy: Signal<bool>,
    status: Signal<Option<String>>,
    status_is_error: Signal<bool>,
}

impl UploadState {
    /// Clear any staged upload + confirm state. Called when the user switches
    /// upload type so an ebook body can't be submitted as an audiobook.
    fn reset_staged(&mut self) {
        self.filename.set(String::new());
        self.file_bytes.set(None);
        self.audio_files.set(Vec::new());
        self.title.set(String::new());
        self.author.set(String::new());
        self.series.set(String::new());
        self.series_index.set(String::new());
        self.inspected.set(false);
        self.status.set(None);
        self.status_is_error.set(false);
    }
}

/// Upload form: pick a file, confirm/correct the auto-extracted metadata, file it.
#[component]
pub fn AddBooksPage() -> Element {
    // All hooks run unconditionally on every render — only the rsx output
    // below branches on `can_upload` — so the hook call order stays stable
    // once the boot effect resolves the real permission (rule 07).
    let can_upload = crate::use_can_upload();
    let server_url = use_server_url();
    let nav = use_navigator();

    let state = UploadState {
        mode: use_signal(|| UploadMode::Ebook),
        filename: use_signal(String::new),
        file_bytes: use_signal(|| None),
        audio_files: use_signal(Vec::new),
        title: use_signal(String::new),
        author: use_signal(String::new),
        series: use_signal(String::new),
        series_index: use_signal(String::new),
        inspected: use_signal(|| false),
        busy: use_signal(|| false),
        status: use_signal(|| None),
        status_is_error: use_signal(|| false),
    };

    let on_file = make_on_file(server_url.clone(), state);
    let on_submit = make_on_submit(server_url, state, nav);
    let is_audiobook = (state.mode)() == UploadMode::Audiobook;

    if !can_upload() {
        return rsx! { AddBooksForbidden {} };
    }

    rsx! {
        section { class: "card",
            h1 { "Add books" }
            p { class: "subtitle",
                "Upload an EPUB or audiobook and Omnibus will file it into your library."
            }

            UploadTypeToggle { state }

            FileDropZone {
                state,
                on_file: EventHandler::new(on_file),
            }

            if (state.inspected)() {
                ConfirmForm {
                    state,
                    audiobook: is_audiobook,
                    on_submit: EventHandler::new(on_submit),
                }
            }

            if let Some(msg) = (state.status)() {
                p {
                    id: "add-books-status",
                    "data-testid": "add-books-status",
                    role: "status",
                    class: if (state.status_is_error)() { "settings-status error" } else { "settings-status success" },
                    "{msg}"
                }
            }
        }
    }
}

/// Not-authorized state shown in place of the form to a user without
/// `can_upload`. Split out (no props, no hooks) so it's directly
/// render-testable without `AddBooksPage`'s router dependency
/// (`use_navigator` panics outside a `Router` ancestor).
#[component]
fn AddBooksForbidden() -> Element {
    rsx! {
        section { class: "card",
            h1 { "Add books" }
            p { class: "settings-status error", "data-testid": "add-books-forbidden",
                "You don't have permission to add books to this library."
            }
        }
    }
}

/// Build the file-select handler: read bytes → inspect → pre-fill the fields.
/// Dispatches on the current [`UploadMode`]: one EPUB, or one-or-more audiobook
/// parts.
fn make_on_file(server_url: String, state: UploadState) -> impl FnMut(Event<FormData>) {
    move |evt: Event<FormData>| {
        if (state.mode)() == UploadMode::Audiobook {
            inspect_audiobook_files(server_url.clone(), state, evt);
        } else {
            inspect_ebook_file(server_url.clone(), state, evt);
        }
    }
}

/// Read the single selected EPUB, inspect it, and pre-fill the confirm form.
fn inspect_ebook_file(server_url: String, state: UploadState, evt: Event<FormData>) {
    let mut s = state;
    let Some(file) = evt.files().into_iter().next() else {
        return;
    };
    let name = file.name();
    s.busy.set(true);
    s.status.set(Some(format!("Reading {name}\u{2026}")));
    s.status_is_error.set(false);
    spawn(async move {
        match file.read_bytes().await {
            Ok(bytes) => {
                let bytes = bytes.to_vec();
                match data::inspect_ebook(&server_url, name.clone(), &bytes).await {
                    Ok(insp) => {
                        s.title.set(insp.title.unwrap_or_default());
                        s.author.set(insp.author.unwrap_or_default());
                        s.series.set(insp.series.unwrap_or_default());
                        s.series_index.set(insp.series_index.unwrap_or_default());
                        s.filename.set(name);
                        s.file_bytes.set(Some(bytes));
                        s.inspected.set(true);
                        s.status
                            .set(Some("Review the details, then add to your library.".into()));
                        s.status_is_error.set(false);
                    }
                    Err(e) => {
                        clear_stage(&mut s);
                        s.status.set(Some(format!("Could not read that EPUB: {e}")));
                        s.status_is_error.set(true);
                    }
                }
            }
            Err(e) => {
                clear_stage(&mut s);
                s.status.set(Some(format!("Could not read that file: {e}")));
                s.status_is_error.set(true);
            }
        }
        s.busy.set(false);
    });
}

/// Read every selected audiobook part, inspect the set, and pre-fill the
/// confirm form.
fn inspect_audiobook_files(server_url: String, state: UploadState, evt: Event<FormData>) {
    let mut s = state;
    let picked: Vec<_> = evt.files().into_iter().collect();
    if picked.is_empty() {
        return;
    }
    let count = picked.len();
    s.busy.set(true);
    s.status.set(Some(format!(
        "Reading {count} file{}\u{2026}",
        if count == 1 { "" } else { "s" }
    )));
    s.status_is_error.set(false);
    spawn(async move {
        let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(count);
        for file in picked {
            let name = file.name();
            match file.read_bytes().await {
                Ok(bytes) => files.push((name, bytes.to_vec())),
                Err(e) => {
                    clear_stage(&mut s);
                    s.status.set(Some(format!("Could not read {name}: {e}")));
                    s.status_is_error.set(true);
                    s.busy.set(false);
                    return;
                }
            }
        }
        match data::inspect_audiobook(&server_url, &files).await {
            Ok(insp) => {
                s.title.set(insp.title.unwrap_or_default());
                s.author.set(insp.author.unwrap_or_default());
                s.filename.set(audiobook_summary(&files));
                s.audio_files.set(files);
                s.inspected.set(true);
                s.status
                    .set(Some("Review the details, then add to your library.".into()));
                s.status_is_error.set(false);
            }
            Err(e) => {
                clear_stage(&mut s);
                s.status
                    .set(Some(format!("Could not read that audiobook: {e}")));
                s.status_is_error.set(true);
            }
        }
        s.busy.set(false);
    });
}

/// Human-readable label for the staged audiobook part(s) in the drop zone.
fn audiobook_summary(files: &[(String, Vec<u8>)]) -> String {
    match files {
        [(name, _)] => name.clone(),
        _ => format!("{} parts selected", files.len()),
    }
}

/// Clear any previously-staged upload so stale bytes can't be submitted after a
/// new pick fails inspect.
fn clear_stage(s: &mut UploadState) {
    s.inspected.set(false);
    s.file_bytes.set(None);
    s.audio_files.set(Vec::new());
    s.filename.set(String::new());
}

/// Build the confirm handler: validate → file the book → redirect to it.
fn make_on_submit(
    server_url: String,
    state: UploadState,
    nav: dioxus_router::Navigator,
) -> impl FnMut(FormEvent) {
    move |evt: FormEvent| {
        evt.prevent_default();
        if (state.mode)() == UploadMode::Audiobook {
            submit_audiobook(server_url.clone(), state, nav);
        } else {
            submit_ebook(server_url.clone(), state, nav);
        }
    }
}

/// Validate + commit the staged EPUB, then navigate to the new book.
fn submit_ebook(server_url: String, state: UploadState, nav: dioxus_router::Navigator) {
    let mut s = state;
    let confirmed_title = (s.title)().trim().to_string();
    let confirmed_author = (s.author)().trim().to_string();
    if confirmed_title.is_empty() || confirmed_author.is_empty() {
        s.status.set(Some("Title and author are required.".into()));
        s.status_is_error.set(true);
        return;
    }
    let Some(bytes) = (s.file_bytes)() else {
        s.status.set(Some("Choose an EPUB file first.".into()));
        s.status_is_error.set(true);
        return;
    };
    let name = (s.filename)();
    let meta = EbookUploadMeta {
        title: confirmed_title,
        author: confirmed_author,
        series: (s.series)().trim().to_string(),
        series_index: (s.series_index)().trim().to_string(),
    };
    s.busy.set(true);
    s.status.set(Some("Adding to your library\u{2026}".into()));
    s.status_is_error.set(false);
    spawn(async move {
        match data::upload_ebook(&server_url, name, bytes, meta).await {
            Ok(result) => {
                nav.push(Route::BookDetail { uuid: result.uuid });
            }
            Err(e) => {
                s.status.set(Some(format!("Upload failed: {e}")));
                s.status_is_error.set(true);
                s.busy.set(false);
            }
        }
    });
}

/// Validate + commit the staged audiobook part(s), then navigate to the new book.
fn submit_audiobook(server_url: String, state: UploadState, nav: dioxus_router::Navigator) {
    let mut s = state;
    let confirmed_title = (s.title)().trim().to_string();
    let confirmed_author = (s.author)().trim().to_string();
    if confirmed_title.is_empty() || confirmed_author.is_empty() {
        s.status.set(Some("Title and author are required.".into()));
        s.status_is_error.set(true);
        return;
    }
    let files = (s.audio_files)();
    if files.is_empty() {
        s.status.set(Some("Choose an audiobook file first.".into()));
        s.status_is_error.set(true);
        return;
    }
    let meta = AudiobookUploadMeta {
        title: confirmed_title,
        author: confirmed_author,
    };
    s.busy.set(true);
    s.status.set(Some("Adding to your library\u{2026}".into()));
    s.status_is_error.set(false);
    spawn(async move {
        match data::upload_audiobook(&server_url, files, meta).await {
            Ok(result) => {
                nav.push(Route::BookDetail { uuid: result.uuid });
            }
            Err(e) => {
                s.status.set(Some(format!("Upload failed: {e}")));
                s.status_is_error.set(true);
                s.busy.set(false);
            }
        }
    });
}

/// Ebook / audiobook type toggle. Switching type clears any staged upload.
#[component]
fn UploadTypeToggle(state: UploadState) -> Element {
    let mut s = state;
    let mode = (state.mode)();
    let busy = (state.busy)();
    rsx! {
        div {
            class: "add-books-types",
            role: "group",
            aria_label: "Upload type",
            button {
                r#type: "button",
                class: if mode == UploadMode::Ebook { "btn" } else { "btn ghost" },
                aria_pressed: if mode == UploadMode::Ebook { "true" } else { "false" },
                disabled: busy,
                "data-testid": "add-books-type-ebook",
                onclick: move |_| {
                    if (s.mode)() != UploadMode::Ebook {
                        s.mode.set(UploadMode::Ebook);
                        s.reset_staged();
                    }
                },
                "Ebook"
            }
            button {
                r#type: "button",
                class: if mode == UploadMode::Audiobook { "btn" } else { "btn ghost" },
                aria_pressed: if mode == UploadMode::Audiobook { "true" } else { "false" },
                disabled: busy,
                "data-testid": "add-books-type-audiobook",
                onclick: move |_| {
                    if (s.mode)() != UploadMode::Audiobook {
                        s.mode.set(UploadMode::Audiobook);
                        s.reset_staged();
                    }
                },
                "Audiobook"
            }
        }
    }
}

/// File-picker drop zone: prompt icon when empty, filename + checkmark once
/// chosen. Accepts a single EPUB or one-or-more audiobook parts per mode.
#[component]
fn FileDropZone(state: UploadState, on_file: EventHandler<Event<FormData>>) -> Element {
    let filename = state.filename;
    let busy = state.busy;
    let audiobook = (state.mode)() == UploadMode::Audiobook;
    let (label, accept, prompt) = if audiobook {
        (
            "Audiobook files",
            ".m4a,.m4b,.mp3,audio/mp4,audio/mpeg",
            "Drop .m4b/.m4a or .mp3 parts here or ",
        )
    } else {
        (
            "EPUB file",
            ".epub,application/epub+zip",
            "Drop an EPUB here or ",
        )
    };
    rsx! {
        div { class: "settings-field",
            span { class: "settings-label", "{label}" }
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
                            "{prompt}"
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
                    accept,
                    multiple: audiobook,
                    "data-testid": "add-books-file-input",
                    aria_label: "{label}",
                    class: "file-drop-input",
                    disabled: busy(),
                    onchange: move |evt| on_file.call(evt),
                }
            }
        }
    }
}

/// Editable confirm form shown after a successful inspect. Audiobooks omit the
/// series fields (the audiobook parser doesn't extract them).
#[component]
fn ConfirmForm(state: UploadState, audiobook: bool, on_submit: EventHandler<FormEvent>) -> Element {
    let mut title = state.title;
    let mut author = state.author;
    let mut series = state.series;
    let mut series_index = state.series_index;
    let busy = state.busy;
    rsx! {
        form {
            id: "add-books-form",
            class: "settings-form",
            onsubmit: move |evt| on_submit.call(evt),

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
            if !audiobook {
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
}

#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;

    /// A user without `can_upload` sees the not-authorized state, not the
    /// upload form — the markup `AddBooksPage` returns via `AddBooksForbidden`
    /// when its `use_can_upload` gate is (the SSR/pre-hydration default) false.
    #[test]
    fn add_books_forbidden_renders_not_authorized_message_not_the_form() {
        let html = dioxus::ssr::render_element(rsx! { AddBooksForbidden {} });
        assert!(html.contains("data-testid=\"add-books-forbidden\""));
        assert!(html.contains("have permission to add books"));
        assert!(!html.contains("add-books-file-input"));
        assert!(!html.contains("add-books-submit"));
    }
}
