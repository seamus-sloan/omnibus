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

#[component]
fn AuthorPhotoEditModal(
    author_id: i64,
    author_name: String,
    server_url: String,
    on_close: EventHandler<()>,
    on_change: EventHandler<()>,
) -> Element {
    let mut url_input = use_signal(String::new);
    let mut status: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);

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
                    p { class: "subtitle author-photo-modal__sub", "{author_name}" }
                }

                // Option 1: paste image URL.
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
                            onclick: {
                                let server_url = server_url.clone();
                                move |_| {
                                    let server_url = server_url.clone();
                                    let url = url_input.read().trim().to_string();
                                    if url.is_empty() {
                                        return;
                                    }
                                    busy.set(true);
                                    status.set(Some("Fetching\u{2026}".into()));
                                    spawn(async move {
                                        match data::set_author_photo_url(&server_url, author_id, url).await {
                                            Ok(()) => {
                                                status.set(Some("Photo updated.".into()));
                                                on_change.call(());
                                            }
                                            Err(e) => status.set(Some(format!("URL failed: {e}"))),
                                        }
                                        busy.set(false);
                                    });
                                }
                            },
                            "Use URL"
                        }
                    }
                }

                // Option 2: upload file.
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
                        onchange: {
                            let server_url = server_url.clone();
                            move |evt: Event<FormData>| {
                                let server_url = server_url.clone();
                                let Some(file) = evt.files().into_iter().next() else { return };
                                let filename = file.name();
                                let mime = file
                                    .content_type()
                                    .unwrap_or_else(|| "application/octet-stream".into());
                                busy.set(true);
                                status.set(Some(format!("Uploading {filename}\u{2026}")));
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
                                                    status.set(Some("Photo uploaded.".into()));
                                                    on_change.call(());
                                                }
                                                Err(e) => {
                                                    status.set(Some(format!("Upload failed: {e}")))
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            status.set(Some(format!("Read file failed: {e}")))
                                        }
                                    }
                                    busy.set(false);
                                });
                            }
                        },
                    }
                }

                // Option 3: scan Open Library.
                section { class: "author-photo-modal__section",
                    label { class: "label", "Or fetch automatically" }
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: busy(),
                        "data-testid": "author-photo-scan",
                        onclick: {
                            let server_url = server_url.clone();
                            move |_| {
                                let server_url = server_url.clone();
                                busy.set(true);
                                status.set(Some("Scanning Open Library\u{2026}".into()));
                                spawn(async move {
                                    match data::scan_author_photo(&server_url, author_id).await {
                                        Ok(r) if r.resolved => {
                                            status.set(Some("Photo found.".into()));
                                            on_change.call(());
                                        }
                                        Ok(_) => status.set(Some(
                                            "No photo on Open Library for this author.".into(),
                                        )),
                                        Err(e) => status.set(Some(format!("Scan failed: {e}"))),
                                    }
                                    busy.set(false);
                                });
                            }
                        },
                        "Scan for picture"
                    }
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
