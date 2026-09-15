//! Add-books page (`/add-books`) — upload an EPUB or audiobook into the
//! library, gated on `can_upload` (server's `require_upload` remains the real
//! boundary). One picker for both: the file extensions decide which ingest
//! the pick goes to, the server parses it for an editable confirm step, then
//! files it into the canonical folder and redirects to the new book. rsx is
//! target-agnostic — file interop runs only in `spawn`.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{AudiobookInspection, UploadInspection};

use crate::data::{self, AudiobookUploadMeta, EbookUploadMeta};
use crate::{use_server_url, Route};

/// Which ingest a pick goes to, decided by [`classify_pick`] from the file
/// extensions — never chosen by the user. Drives which data-layer call the
/// inspect and submit handlers make.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum UploadKind {
    Ebook,
    Audiobook,
}

/// Extensions the ebook ingest takes (`/api/uploads/ebooks`), matching the
/// server's `detect_ebook_format`.
const EBOOK_EXTENSIONS: &[&str] = &["epub", "pdf"];
/// Extensions the audiobook ingest takes (`/api/uploads/audiobooks`), matching
/// the server's `audiobook_ext_of`.
const AUDIOBOOK_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp4", "mp3"];
/// The picker's `accept` list: every extension above plus their MIME types,
/// so a browser filters the dialog without the page having to.
const ACCEPT: &str =
    ".epub,.pdf,.m4b,.m4a,.mp4,.mp3,application/epub+zip,application/pdf,audio/mp4,audio/mpeg";

/// Decide which ingest a set of picked filenames goes to, or say why it can't.
///
/// One EPUB or PDF is an ebook; any number of audiobook files is an audiobook (the
/// server still rejects two `.m4b`s or a mixed set of its own — this only
/// routes). Everything else is refused here so the wrong endpoint is never
/// asked: a mix of the two, several EPUBs, or an extension neither takes.
fn classify_pick(names: &[String]) -> Result<UploadKind, String> {
    let ext_of = |name: &String| {
        std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    let mut ebooks = 0usize;
    let mut audio = 0usize;
    for name in names {
        let ext = ext_of(name);
        if EBOOK_EXTENSIONS.contains(&ext.as_str()) {
            ebooks += 1;
        } else if AUDIOBOOK_EXTENSIONS.contains(&ext.as_str()) {
            audio += 1;
        } else {
            return Err(format!(
                "{name} isn't a format Omnibus can add — pick an EPUB or PDF, an M4B/M4A/MP4 audiobook, or MP3 parts."
            ));
        }
    }
    match (ebooks, audio) {
        (0, 0) => Err("Choose a file first.".into()),
        (1, 0) => Ok(UploadKind::Ebook),
        (0, _) => Ok(UploadKind::Audiobook),
        (_, 0) => Err("Add one EPUB at a time.".into()),
        _ => Err("Pick either an EPUB or an audiobook's files, not both.".into()),
    }
}

/// The editable-metadata + staged-upload signals threaded through the page's
/// handlers and confirm form. `Copy` so the async handlers can capture them.
#[derive(Copy, Clone, PartialEq)]
struct UploadState {
    /// Which ingest the staged pick belongs to; `None` until a pick classifies.
    kind: Signal<Option<UploadKind>>,
    filename: Signal<String>,
    /// Staged EPUB bytes (an ebook pick).
    file_bytes: Signal<Option<Vec<u8>>>,
    /// Staged audiobook part(s) as `(filename, bytes)` (an audiobook pick).
    audio_files: Signal<Vec<(String, Vec<u8>)>>,
    title: Signal<String>,
    author: Signal<String>,
    /// Creators after the first, as the file declared them (#2355). Listed
    /// under the Author field and kept on commit; never edited here.
    more_creators: Signal<Vec<String>>,
    series: Signal<String>,
    series_index: Signal<String>,
    inspected: Signal<bool>,
    busy: Signal<bool>,
    status: Signal<Option<String>>,
    status_is_error: Signal<bool>,
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
        kind: use_signal(|| None),
        filename: use_signal(String::new),
        file_bytes: use_signal(|| None),
        audio_files: use_signal(Vec::new),
        title: use_signal(String::new),
        author: use_signal(String::new),
        more_creators: use_signal(Vec::new),
        series: use_signal(String::new),
        series_index: use_signal(String::new),
        inspected: use_signal(|| false),
        busy: use_signal(|| false),
        status: use_signal(|| None),
        status_is_error: use_signal(|| false),
    };

    let on_file = make_on_file(server_url.clone(), state);
    let on_submit = make_on_submit(server_url, state, nav);

    if !can_upload() {
        return rsx! { AddBooksForbidden {} };
    }

    rsx! {
        section { class: "card",
            h1 { "Upload a book" }

            FileDropZone {
                state,
                on_file: EventHandler::new(on_file),
            }

            if (state.inspected)() {
                ConfirmForm {
                    state,
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
            h1 { "Upload a book" }
            p { class: "settings-status error", "data-testid": "add-books-forbidden",
                "You don't have permission to add books to this library."
            }
        }
    }
}

/// Build the file-select handler: classify the pick by extension, then read
/// bytes → inspect → pre-fill the fields on the ingest it belongs to. A pick
/// that fits neither is refused here with the reason, and nothing is sent.
fn make_on_file(server_url: String, state: UploadState) -> impl FnMut(Event<FormData>) {
    move |evt: Event<FormData>| {
        let mut s = state;
        let names: Vec<String> = evt.files().iter().map(|f| f.name()).collect();
        if names.is_empty() {
            return;
        }
        match classify_pick(&names) {
            Ok(UploadKind::Ebook) => {
                s.kind.set(Some(UploadKind::Ebook));
                inspect_ebook_file(server_url.clone(), state, evt);
            }
            Ok(UploadKind::Audiobook) => {
                s.kind.set(Some(UploadKind::Audiobook));
                inspect_audiobook_files(server_url.clone(), state, evt);
            }
            Err(reason) => {
                clear_stage(&mut s);
                s.status.set(Some(reason));
                s.status_is_error.set(true);
            }
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
                        prefill_from_ebook(&mut s, insp);
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
                prefill_from_audiobook(&mut s, insp);
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

/// Pre-fill every confirm field from an EPUB inspection.
fn prefill_from_ebook(s: &mut UploadState, insp: UploadInspection) {
    s.title.set(insp.title.unwrap_or_default());
    s.author.set(insp.author.unwrap_or_default());
    s.more_creators
        .set(insp.creators.iter().skip(1).cloned().collect());
    s.series.set(insp.series.unwrap_or_default());
    s.series_index.set(insp.series_index.unwrap_or_default());
}

/// Pre-fill every confirm field from an audiobook inspection. The parser
/// reports no series, so the fields are cleared rather than left alone: with
/// one picker there is no type switch to reset them, and the previous pick's
/// series would otherwise be committed with this book.
fn prefill_from_audiobook(s: &mut UploadState, insp: AudiobookInspection) {
    s.title.set(insp.title.unwrap_or_default());
    s.author.set(insp.author.unwrap_or_default());
    s.more_creators
        .set(insp.creators.iter().skip(1).cloned().collect());
    s.series.set(String::new());
    s.series_index.set(String::new());
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
    s.kind.set(None);
    s.inspected.set(false);
    s.file_bytes.set(None);
    s.audio_files.set(Vec::new());
    s.more_creators.set(Vec::new());
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
        match (state.kind)() {
            Some(UploadKind::Audiobook) => submit_audiobook(server_url.clone(), state, nav),
            Some(UploadKind::Ebook) => submit_ebook(server_url.clone(), state, nav),
            None => {
                let mut s = state;
                s.status.set(Some("Choose a file first.".into()));
                s.status_is_error.set(true);
            }
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
        series: (s.series)().trim().to_string(),
        series_index: (s.series_index)().trim().to_string(),
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

/// File-picker drop zone: prompt icon when empty, filename + checkmark once
/// chosen. One picker for every format — a single EPUB, a single audiobook
/// container, or the `.mp3` parts of one book — sorted out by extension after
/// the pick, so it always allows a multi-select.
#[component]
fn FileDropZone(state: UploadState, on_file: EventHandler<Event<FormData>>) -> Element {
    let filename = state.filename;
    let busy = state.busy;
    rsx! {
        div { class: "settings-field",
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
                            "Drop an EPUB or audiobook here or "
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
                    accept: ACCEPT,
                    multiple: true,
                    "data-testid": "add-books-file-input",
                    aria_label: "Book files",
                    class: "file-drop-input",
                    disabled: busy(),
                    onchange: move |evt| on_file.call(evt),
                }
            }
            p {
                class: "settings-hint",
                "data-testid": "add-books-formats",
                "EPUB, M4B, M4A, MP4, or the MP3 parts of one audiobook."
            }
        }
    }
}

/// Editable confirm form shown after a successful inspect. Both upload types
/// offer the series fields: the audiobook parser usually extracts nothing, and
/// this is the only point in the flow where a series can be supplied.
#[component]
fn ConfirmForm(state: UploadState, on_submit: EventHandler<FormEvent>) -> Element {
    let mut title = state.title;
    let mut author = state.author;
    let mut series = state.series;
    let mut series_index = state.series_index;
    let busy = state.busy;
    // The form under-reported what it was about to save when a file named
    // several creators (#2355): name the rest, and say what editing does.
    let also_credited = (state.more_creators)().join(", ");
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
                if !also_credited.is_empty() {
                    p {
                        class: "settings-hint",
                        "data-testid": "add-books-more-creators",
                        "Also credited: {also_credited}. Kept as additional creators — editing Author replaces only the first name."
                    }
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
}

#[cfg(test)]
mod tests;
