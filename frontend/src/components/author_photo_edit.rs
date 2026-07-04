//! Author photo edit affordance: a hover-revealed pencil button overlaid on
//! the existing avatar that opens a modal with three actions —
//!
//! 1. **Paste image URL** — server fetches and validates the URL, persists
//!    as a `manual` photo.
//! 2. **Upload file** — multipart body to the same `PUT /api/authors/:id/photo`
//!    endpoint used by the original admin upload path.
//! 3. **Scan for picture** — re-runs the Open Library cascade.
//!
//! Used by `pages::author::AuthorPage` (hero avatar) and
//! `pages::authors_index::AuthorsIndexPage` (per-card avatar). Both wrap
//! their `<img>`/`<div>` inside `<AuthorPhotoEditOverlay>` so the same hover
//! affordance shows up wherever the photo is rendered.
//!
//! The modal is web-only. SSR renders the wrapper but no overlay, and the
//! mobile shell doesn't mount this component — mobile uses the discovery
//! screens read-only for now.

use dioxus::prelude::*;

use crate::data;

/// Wrapper that renders an "edit" pencil button positioned over its child.
/// The button is hidden by default and revealed on hover/focus (via the
/// `author-photo-edit__overlay` CSS rule). Clicking opens the modal.
///
/// `on_change` fires after a successful URL/upload/scan so the parent can
/// re-fetch the author payload and swap in the new photo.
#[component]
pub fn AuthorPhotoEditOverlay(
    author_id: i64,
    author_name: String,
    server_url: String,
    on_change: EventHandler<()>,
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);

    rsx! {
        div { class: "author-photo-edit",
            {children}
            button {
                r#type: "button",
                class: "author-photo-edit__overlay",
                aria_label: "Edit photo for {author_name}",
                "data-testid": "author-photo-edit",
                // The overlay sits on top of the avatar — which on the
                // index card is wrapped in a `Link`. Stop propagation
                // AND prevent the browser's default click-on-anchor
                // navigation so clicking the pencil doesn't also navigate
                // to the author detail page. `stop_propagation` alone
                // wouldn't be enough — dioxus_router's Link intercepts
                // the click in the capture phase, and the browser still
                // honours the `<a href>` default action when a click is
                // dispatched inside the link's subtree.
                onclick: move |evt| {
                    evt.stop_propagation();
                    evt.prevent_default();
                    open.set(true);
                },
                "Edit"
            }
            if open() {
                AuthorPhotoEditModal {
                    author_id,
                    author_name: author_name.clone(),
                    server_url: server_url.clone(),
                    on_close: move |_| open.set(false),
                    on_change: move |_| {
                        on_change.call(());
                        open.set(false);
                    },
                }
            }
        }
    }
}

/// In-flight-request flag plus the last status/error message, shared by
/// all three photo-source sections so only one can run — and report — at
/// a time.
#[derive(Clone, Copy, PartialEq)]
struct PhotoEditStatus {
    busy: Signal<bool>,
    status: Signal<Option<String>>,
}

#[component]
fn AuthorPhotoEditModal(
    author_id: i64,
    author_name: String,
    server_url: String,
    on_close: EventHandler<()>,
    on_change: EventHandler<()>,
) -> Element {
    let url_input = use_signal(String::new);
    let busy = use_signal(|| false);
    let status: Signal<Option<String>> = use_signal(|| None);
    let edit_status = PhotoEditStatus { busy, status };

    rsx! {
        div {
            class: "author-photo-modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            aria_label: "Edit photo for {author_name}",
            // The overlay is rendered as a child of the author card's
            // `<Link>`, so any unstopped click would also navigate to the
            // detail page. Stop propagation on every modal-level click.
            onclick: move |evt| {
                evt.stop_propagation();
                on_close.call(());
            },
            div {
                class: "author-photo-modal",
                // Clicks inside the modal body don't close it (they don't
                // reach the backdrop) and don't navigate (propagation
                // stops here).
                onclick: move |evt| evt.stop_propagation(),

                div { class: "author-photo-modal__head",
                    h2 { class: "author-photo-modal__title", "Edit photo" }
                }

                UrlPhotoSection {
                    author_id,
                    server_url: server_url.clone(),
                    url_input,
                    status: edit_status,
                    on_change,
                }
                FilePhotoSection {
                    author_id,
                    server_url: server_url.clone(),
                    status: edit_status,
                    on_change,
                }
                ScanPhotoSection {
                    author_id,
                    server_url: server_url.clone(),
                    status: edit_status,
                    on_change,
                }

                if let Some(msg) = status() {
                    p {
                        class: "author-photo-modal__status",
                        role: "status",
                        "data-testid": "author-photo-status",
                        "{msg}"
                    }
                }

                div { class: "author-photo-modal__actions",
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
        }
    }
}

/// Option 1: paste image URL — server fetches and validates it, then
/// persists the result as a `manual` photo.
#[component]
fn UrlPhotoSection(
    author_id: i64,
    server_url: String,
    mut url_input: Signal<String>,
    status: PhotoEditStatus,
    on_change: EventHandler<()>,
) -> Element {
    let mut busy = status.busy;
    let mut msg = status.status;

    rsx! {
        section { class: "author-photo-modal__section",
            label { class: "label", r#for: "author-photo-url",
                "Paste image URL"
            }
            div { class: "author-photo-modal__url-row",
                input {
                    id: "author-photo-url",
                    class: "me-input",
                    r#type: "url",
                    placeholder: "https://example.com/photo.jpg",
                    "data-testid": "author-photo-url-input",
                    disabled: busy(),
                    value: "{url_input}",
                    oninput: move |e| url_input.set(e.value()),
                }
                button {
                    r#type: "button",
                    class: "btn",
                    disabled: busy() || url_input.read().trim().is_empty(),
                    "data-testid": "author-photo-url-submit",
                    onclick: move |_| {
                        let server_url = server_url.clone();
                        let url = url_input.read().trim().to_string();
                        if url.is_empty() {
                            return;
                        }
                        busy.set(true);
                        msg.set(Some("Fetching\u{2026}".into()));
                        spawn(async move {
                            match data::set_author_photo_url(&server_url, author_id, url).await {
                                Ok(()) => {
                                    msg.set(Some("Photo updated.".into()));
                                    on_change.call(());
                                }
                                Err(e) => msg.set(Some(format!("URL failed: {e}"))),
                            }
                            busy.set(false);
                        });
                    },
                    "Use URL"
                }
            }
        }
    }
}

/// Option 2: upload a file from this device — sent as a multipart body
/// to the same `PUT /api/authors/:id/photo` endpoint the URL/scan
/// actions hit.
#[component]
fn FilePhotoSection(
    author_id: i64,
    server_url: String,
    status: PhotoEditStatus,
    on_change: EventHandler<()>,
) -> Element {
    let mut busy = status.busy;
    let mut msg = status.status;

    rsx! {
        section { class: "author-photo-modal__section",
            label { class: "label", r#for: "author-photo-file",
                "Upload from this device"
            }
            input {
                id: "author-photo-file",
                class: "author-photo-modal__file",
                r#type: "file",
                accept: "image/jpeg,image/png,image/webp,image/gif",
                "data-testid": "author-photo-file-input",
                disabled: busy(),
                onchange: move |evt: Event<FormData>| {
                    let server_url = server_url.clone();
                    let Some(file) = evt.files().into_iter().next() else { return };
                    let filename = file.name();
                    let mime = file
                        .content_type()
                        .unwrap_or_else(|| "application/octet-stream".into());
                    busy.set(true);
                    msg.set(Some(format!("Uploading {filename}\u{2026}")));
                    spawn(async move {
                        match file.read_bytes().await {
                            Ok(bytes) => {
                                match data::upload_author_photo(
                                    &server_url,
                                    author_id,
                                    filename,
                                    mime,
                                    bytes.to_vec(),
                                )
                                .await
                                {
                                    Ok(()) => {
                                        msg.set(Some("Photo uploaded.".into()));
                                        on_change.call(());
                                    }
                                    Err(e) => msg.set(Some(format!("Upload failed: {e}"))),
                                }
                            }
                            Err(e) => msg.set(Some(format!("Read file failed: {e}"))),
                        }
                        busy.set(false);
                    });
                },
            }
        }
    }
}

/// Option 3: scan Open Library — re-runs the same author-photo cascade
/// the library scanner uses.
#[component]
fn ScanPhotoSection(
    author_id: i64,
    server_url: String,
    status: PhotoEditStatus,
    on_change: EventHandler<()>,
) -> Element {
    let mut busy = status.busy;
    let mut msg = status.status;

    rsx! {
        section { class: "author-photo-modal__section",
            label { class: "label", "Or fetch automatically" }
            button {
                r#type: "button",
                class: "btn ghost",
                disabled: busy(),
                "data-testid": "author-photo-scan",
                onclick: move |_| {
                    let server_url = server_url.clone();
                    busy.set(true);
                    msg.set(Some("Scanning Open Library\u{2026}".into()));
                    spawn(async move {
                        match data::scan_author_photo(&server_url, author_id).await {
                            Ok(r) if r.resolved => {
                                msg.set(Some("Photo found.".into()));
                                on_change.call(());
                            }
                            Ok(_) => {
                                msg.set(Some("No photo on Open Library for this author.".into()))
                            }
                            Err(e) => msg.set(Some(format!("Scan failed: {e}"))),
                        }
                        busy.set(false);
                    });
                },
                "Scan for picture"
            }
        }
    }
}
