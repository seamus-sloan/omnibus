//! Admin-only "Delete files…" dialog mounted by the book detail page. Step 1
//! lists every deletable *item* (each `book_files` row and physical copy) with
//! a checkbox; step 2 confirms, with copy branching on partial vs total. Its
//! manifest is fetched on mount rather than read off the page's
//! `EbookMetadata`, which only carries `book_files` for multi-file formats.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use omnibus_shared::physical::PhysicalCopy;
use omnibus_shared::{BookDeletionManifest, BookFileInfo, DeleteBookFilesResult};

use crate::components::ConfirmModal;
use crate::{data, format, use_server_url};

/// Which pane the dialog is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Pick the items to delete.
    Choose,
    /// Confirm the picked set.
    Confirm,
}

/// Dialog state for one mount: the picked file/copy ids, which pane is
/// showing, the fetched manifest, the last error, and the in-flight flag.
#[derive(Clone, Copy)]
struct DeleteDialogSignals {
    files: Signal<BTreeSet<i64>>,
    copies: Signal<BTreeSet<i64>>,
    stage: Signal<Stage>,
    manifest: Signal<Option<BookDeletionManifest>>,
    error: Signal<Option<String>>,
    busy: Signal<bool>,
}

/// Fresh signal set for one dialog mount.
fn use_delete_dialog_signals() -> DeleteDialogSignals {
    DeleteDialogSignals {
        files: use_signal(BTreeSet::new),
        copies: use_signal(BTreeSet::new),
        stage: use_signal(|| Stage::Choose),
        manifest: use_signal(|| None),
        error: use_signal(|| None),
        busy: use_signal(|| false),
    }
}

/// Modal dialog: item checklist → confirm → delete.
#[component]
pub fn DeleteBookDialog(
    uuid: String,
    title: String,
    on_deleted: EventHandler<DeleteBookFilesResult>,
    on_close: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let signals = use_delete_dialog_signals();
    let busy = signals.busy;
    let do_delete = build_do_delete(server_url.clone(), uuid.clone(), signals, on_deleted);

    use_effect({
        let url = server_url.clone();
        let uuid = uuid.clone();
        let mut manifest = signals.manifest;
        let mut error = signals.error;
        move || {
            let (url, uuid) = (url.clone(), uuid.clone());
            spawn(async move {
                match data::book_deletion_manifest(&url, &uuid).await {
                    Ok(m) => manifest.set(Some(m)),
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    });

    rsx! {
        // No dismissal mid-delete — closing would hide the outcome while
        // the request keeps running (same rule as the merge dialog).
        ConfirmModal {
            testid: "delete-book-dialog".to_string(),
            aria_label: "Delete book files".to_string(),
            dialog_class: "mg-modal del-modal".to_string(),
            busy: busy(),
            on_dismiss: move |_| on_close.call(()),
            {render_pane(title, signals, do_delete, on_close)}
        }
    }
}

/// Route to the choose or confirm pane. Until the manifest lands there is
/// nothing to list, so the dialog shows a placeholder rather than guessing at
/// an item count.
fn render_pane(
    title: String,
    signals: DeleteDialogSignals,
    do_delete: impl FnMut(()) + 'static,
    on_close: EventHandler<()>,
) -> Element {
    let Some(manifest) = signals.manifest.read().clone() else {
        return rsx! {
            div { class: "del-body",
                h2 { class: "del-title", "Delete files\u{2026}" }
                p { class: "del-copy", "Loading this book\u{2019}s files\u{2026}" }
                if let Some(msg) = signals.error.read().clone() {
                    p { role: "alert", class: "bd-merge-error", "{msg}" }
                }
                {render_actions(rsx! {}, on_close, signals.busy)}
            }
        };
    };
    if *signals.stage.read() == Stage::Confirm || manifest.item_count() == 0 {
        render_confirm(title, manifest, signals, do_delete, on_close)
    } else {
        render_choose(title, manifest, signals, on_close)
    }
}

/// Confirmed-delete action: guards double-click re-entry, then calls the RPC
/// and forwards the outcome so the page can navigate away or refetch.
fn build_do_delete(
    server_url: String,
    uuid: String,
    signals: DeleteDialogSignals,
    on_deleted: EventHandler<DeleteBookFilesResult>,
) -> impl FnMut(()) + 'static {
    let DeleteDialogSignals {
        files,
        copies,
        mut busy,
        mut error,
        ..
    } = signals;

    move |_| {
        // A double-click can land before the disabled state re-renders.
        if busy() {
            return;
        }
        let url = server_url.clone();
        let uuid = uuid.clone();
        let file_ids: Vec<i64> = files.read().iter().copied().collect();
        let copy_ids: Vec<i64> = copies.read().iter().copied().collect();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::delete_book_files(&url, &uuid, file_ids, copy_ids).await {
                Ok(res) => on_deleted.call(res),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    }
}

/// Step 1 — the item checklist, files first and physical copies below.
fn render_choose(
    title: String,
    manifest: BookDeletionManifest,
    signals: DeleteDialogSignals,
    on_close: EventHandler<()>,
) -> Element {
    let DeleteDialogSignals {
        files,
        copies,
        mut stage,
        ..
    } = signals;
    let picked = files.read().len() + copies.read().len();
    let has_copies = !manifest.copies.is_empty();
    // "1 item…" once physical copies are in play, since not every row is a file.
    let noun = match (has_copies, picked) {
        (true, 1) => "item",
        (true, _) => "items",
        (false, 1) => "file",
        (false, _) => "files",
    };
    // The button is disabled at zero, so it names the action rather than a count.
    let action = if picked == 0 {
        format!("Delete {noun}\u{2026}")
    } else {
        format!("Delete {picked} {noun}\u{2026}")
    };
    let all_picked = picked == manifest.item_count();
    let intro = if has_copies {
        "Choose what to remove. Files are deleted from disk; a physical copy is only un-recorded."
    } else {
        "Choose the files to remove. Each one is deleted from disk and from the library database. This cannot be undone."
    };
    let files_head = match (has_copies, manifest.files.len()) {
        // With copies in play the count would misread as the item total, so
        // the section header drops it (as the design's variant does).
        (true, _) => "FILES ON DISK".to_string(),
        (false, 1) => "1 FILE ON DISK".to_string(),
        (false, n) => format!("{n} FILES ON DISK"),
    };
    let select_all = build_select_all(&manifest, signals, all_picked);

    rsx! {
        div { class: "del-body",
            h2 { class: "del-title", "Delete files from \u{201c}{title}\u{201d}?" }
            p { class: "del-copy", "{intro}" }

            div { class: "del-section-head",
                span { class: "label del-section-label", "{files_head}" }
                button {
                    class: "del-select-all",
                    "data-testid": "delete-select-all",
                    r#type: "button",
                    onclick: select_all,
                    if all_picked { "Clear selection" } else { "Select all" }
                }
            }
            ul { class: "del-items",
                for file in manifest.files.iter() {
                    {render_file_row(file.clone(), files)}
                }
            }

            if has_copies {
                div { class: "del-section-head",
                    span { class: "label del-section-label", "PHYSICAL COPIES" }
                }
                ul { class: "del-items",
                    for copy in manifest.copies.iter() {
                        {render_copy_row(copy.clone(), copies)}
                    }
                }
                p { class: "mono del-note",
                    "Book record is removed only when every item here is selected."
                }
            }

            {render_actions(rsx! {
                button {
                    class: "del-btn-danger",
                    "data-testid": "delete-continue",
                    r#type: "button",
                    disabled: picked == 0,
                    onclick: move |_| stage.set(Stage::Confirm),
                    "{action}"
                }
            }, on_close, signals.busy)}
        }
    }
}

/// Select-all / clear-all across both item lists.
fn build_select_all(
    manifest: &BookDeletionManifest,
    signals: DeleteDialogSignals,
    all_picked: bool,
) -> impl FnMut(Event<MouseData>) + 'static {
    let DeleteDialogSignals {
        mut files,
        mut copies,
        ..
    } = signals;
    let file_ids: Vec<i64> = manifest.files.iter().map(|f| f.id).collect();
    let copy_ids: Vec<i64> = manifest.copies.iter().map(|c| c.id).collect();
    move |_| {
        if all_picked {
            files.write().clear();
            copies.write().clear();
        } else {
            files.write().extend(file_ids.iter().copied());
            copies.write().extend(copy_ids.iter().copied());
        }
    }
}

/// One file row: checkbox, format badge, label, and `size · relative/path`.
fn render_file_row(file: BookFileInfo, picked: Signal<BTreeSet<i64>>) -> Element {
    let label = file
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| file.filename.clone());
    let meta = match (format::file_size(file.size_bytes), file.path.as_deref()) {
        (Some(size), Some(path)) => format!("{size} \u{b7} {path}"),
        (Some(size), None) => size,
        (None, Some(path)) => path.to_string(),
        (None, None) => file.filename.clone(),
    };
    render_item_row(ItemRow {
        id: file.id,
        testid: format!("delete-file-{}", file.id),
        badge: file.format.to_uppercase(),
        label,
        meta,
        picked,
    })
}

/// One physical-copy row. A copy has no file, so its meta line says so —
/// deleting it un-records the copy rather than touching the filesystem.
fn render_copy_row(copy: PhysicalCopy, picked: Signal<BTreeSet<i64>>) -> Element {
    let label = copy
        .note
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Physical copy".to_string());
    let meta = match copy.isbn.as_deref() {
        Some(isbn) => format!("ISBN {isbn} \u{b7} no file on disk"),
        None => "no file on disk".to_string(),
    };
    render_item_row(ItemRow {
        id: copy.id,
        testid: format!("delete-copy-{}", copy.id),
        badge: "OWN".to_string(),
        label,
        meta,
        picked,
    })
}

/// The rendered shape shared by file and physical-copy rows.
struct ItemRow {
    id: i64,
    testid: String,
    badge: String,
    label: String,
    meta: String,
    picked: Signal<BTreeSet<i64>>,
}

/// Checkbox + badge + label + mono meta line, highlighted while selected.
fn render_item_row(row: ItemRow) -> Element {
    let ItemRow {
        id,
        testid,
        badge,
        label,
        meta,
        mut picked,
    } = row;
    let checked = picked.read().contains(&id);
    let class = if checked {
        "del-item is-picked"
    } else {
        "del-item"
    };
    rsx! {
        li { key: "{id}", class: "{class}",
            label { class: "del-item-label",
                input {
                    class: "del-check",
                    r#type: "checkbox",
                    "data-testid": "{testid}",
                    checked,
                    onchange: move |evt| {
                        let mut set = picked.write();
                        if evt.checked() {
                            set.insert(id);
                        } else {
                            set.remove(&id);
                        }
                    },
                }
                div { class: "del-item-copy",
                    div { class: "del-item-head",
                        span { class: "mono del-badge", "{badge}" }
                        span { class: "del-item-title", "{label}" }
                    }
                    div { class: "mono del-item-meta", "{meta}" }
                }
            }
        }
    }
}

/// Step 2 — the confirm pane. Three shapes: no items at all, a partial
/// delete the book survives, and a total delete that takes the book.
fn render_confirm(
    title: String,
    manifest: BookDeletionManifest,
    signals: DeleteDialogSignals,
    on_confirm: impl FnMut(()) + 'static,
    on_close: EventHandler<()>,
) -> Element {
    let DeleteDialogSignals {
        files,
        copies,
        mut stage,
        busy,
        ..
    } = signals;
    let mut on_confirm = on_confirm;
    let picked = files.read().len() + copies.read().len();
    let empty = manifest.item_count() == 0;
    let total = empty || picked == manifest.item_count();
    let remaining = manifest.item_count().saturating_sub(picked);

    // "items" once a physical copy is in the mix — not every row is a file.
    let noun = match (manifest.copies.is_empty(), picked) {
        (true, 1) => "file",
        (true, _) => "files",
        (false, 1) => "item",
        (false, _) => "items",
    };
    let heading = match (empty, total, picked) {
        // "Delete all 1 file?" doesn't read — when the whole book goes and
        // there's only one item, name the book instead, as the no-items case
        // does.
        (true, _, _) | (_, true, 1) => format!("Delete \u{201c}{title}\u{201d}?"),
        (_, true, _) => format!("Delete all {picked} {noun}?"),
        _ => format!("Delete {picked} {noun}?"),
    };
    let copy = if empty {
        "This book has no files on disk. Deleting removes the library record only \u{2014} nothing is deleted from your filesystem.".to_string()
    } else if total {
        format!("Every file for \u{201c}{title}\u{201d} will be deleted from disk, and the book will be removed from your library entirely.")
    } else {
        let left = match (manifest.copies.is_empty(), remaining) {
            (true, 1) => "file",
            (true, _) => "files",
            (false, 1) => "item",
            (false, _) => "items",
        };
        format!("{} will be deleted from disk and removed from this book. {title} stays in your library with its {remaining} remaining {left}.", picked_label(&manifest, signals))
    };
    let action = match (empty, total, noun) {
        (true, _, _) => "Delete record".to_string(),
        (_, true, _) => "Delete book".to_string(),
        (_, _, n) => format!("Delete {n}"),
    };
    // Only a total delete takes the uuid-keyed user data with it.
    let losses = total
        .then(|| manifest.impact.losses())
        .filter(|l| !l.is_empty());

    rsx! {
        div { class: "del-body",
            h2 { class: "del-title", "{heading}" }
            p { class: "del-copy", "data-testid": "delete-confirm-copy", "{copy}" }
            if let Some(losses) = losses {
                p { class: "del-copy del-losses", "data-testid": "delete-losses",
                    "Also deleted: {losses.join(\", \")}."
                }
            }
            if let Some(msg) = signals.error.read().clone() {
                p { role: "alert", class: "bd-merge-error", "{msg}" }
            }
            {render_actions(rsx! {
                if !empty {
                    button {
                        class: "del-btn-ghost",
                        "data-testid": "delete-back",
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| stage.set(Stage::Choose),
                        "Back"
                    }
                }
                button {
                    class: "del-btn-danger",
                    "data-testid": "delete-confirm",
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| on_confirm(()),
                    if busy() { "Deleting\u{2026}" } else { "{action}" }
                }
            }, on_close, signals.busy)}
        }
    }
}

/// Quoted label of the single picked file, for the partial-delete copy;
/// falls back to a plain count when several are picked.
fn picked_label(manifest: &BookDeletionManifest, signals: DeleteDialogSignals) -> String {
    let ids = signals.files.read();
    let single = (ids.len() == 1 && signals.copies.read().is_empty())
        .then(|| manifest.files.iter().find(|f| ids.contains(&f.id)))
        .flatten();
    match single {
        Some(file) => format!(
            "\u{201c}{}\u{201d}",
            file.label
                .clone()
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| file.filename.clone())
        ),
        None => format!("{} selected items", ids.len() + signals.copies.read().len()),
    }
}

/// The pinned action row: Cancel plus whatever the pane's own buttons are.
fn render_actions(buttons: Element, on_close: EventHandler<()>, busy: Signal<bool>) -> Element {
    rsx! {
        div { class: "del-actions",
            button {
                class: "del-btn-ghost",
                "data-testid": "delete-cancel",
                r#type: "button",
                disabled: busy(),
                onclick: move |_| on_close.call(()),
                "Cancel"
            }
            {buttons}
        }
    }
}

// Every test here renders SSR markup, so the whole module is `server`-gated —
// under `web` its contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn file(id: i64, format: &str, label: &str, path: &str) -> BookFileInfo {
        BookFileInfo {
            id,
            format: format.to_string(),
            filename: format!("book.{}", format.to_lowercase()),
            ordinal: 0,
            label: Some(label.to_string()),
            size_bytes: 1_200_000,
            path: Some(path.to_string()),
            validator: None,
        }
    }

    fn copy(id: i64) -> PhysicalCopy {
        PhysicalCopy {
            id,
            book_uuid: "uuid-a".into(),
            isbn: Some("9781635575637".into()),
            added_by_user_id: None,
            checked_in_at: 0,
            note: Some("Hardback".into()),
        }
    }

    /// Renders one pane in isolation. The full dialog mounts a manifest-fetch
    /// effect, which `render_element` can't drive without a live runtime.
    #[component]
    fn ChooseHarness(manifest: BookDeletionManifest) -> Element {
        render_choose(
            "Piranesi".to_string(),
            manifest,
            use_delete_dialog_signals(),
            EventHandler::new(|_| {}),
        )
    }

    #[component]
    fn ConfirmHarness(manifest: BookDeletionManifest) -> Element {
        render_confirm(
            "Piranesi".to_string(),
            manifest,
            use_delete_dialog_signals(),
            |_| {},
            EventHandler::new(|_| {}),
        )
    }

    #[test]
    fn choose_pane_lists_each_file_with_its_badge_size_and_path() {
        let html = dioxus::ssr::render_element(rsx! {
            ChooseHarness {
                manifest: BookDeletionManifest {
                    files: vec![
                        file(1, "epub", "Piranesi (Bloomsbury)", "clarke/piranesi.epub"),
                        file(2, "m4b", "Narrated by Chiwetel Ejiofor", "clarke/piranesi.m4b"),
                    ],
                    ..Default::default()
                },
            }
        });

        assert!(html.contains("Delete files from “Piranesi”?"));
        assert!(html.contains("2 FILES ON DISK"));
        assert!(html.contains("data-testid=\"delete-file-1\""));
        assert!(html.contains("data-testid=\"delete-file-2\""));
        assert!(html.contains("EPUB"));
        assert!(html.contains("1.2 MB · clarke/piranesi.epub"));
    }

    #[test]
    fn choose_pane_adds_a_physical_copies_section_when_the_book_has_one() {
        let html = dioxus::ssr::render_element(rsx! {
            ChooseHarness {
                manifest: BookDeletionManifest {
                    files: vec![file(1, "epub", "Piranesi (Bloomsbury)", "clarke/piranesi.epub")],
                    copies: vec![copy(7)],
                    ..Default::default()
                },
            }
        });

        assert!(html.contains("PHYSICAL COPIES"));
        assert!(html.contains("data-testid=\"delete-copy-7\""));
        assert!(html.contains("ISBN 9781635575637 · no file on disk"));
        assert!(html.contains("Book record is removed only when every item here is selected."));
    }

    #[test]
    fn confirm_pane_offers_the_record_delete_for_a_book_with_no_items() {
        let html = dioxus::ssr::render_element(rsx! {
            ConfirmHarness { manifest: BookDeletionManifest::default() }
        });

        assert!(html.contains("Delete “Piranesi”?"));
        assert!(html.contains("This book has no files on disk."));
        assert!(html.contains("Delete record"));
        // Nothing to go back to when there was no choose step.
        assert!(!html.contains("data-testid=\"delete-back\""));
    }

    /// Mounts the real `DeleteBookDialog`, not just its inner panes — the
    /// manifest fetch is a `use_effect`-spawned future that doesn't resolve
    /// within one `render_in_vdom` rebuild, so this exercises the dialog's
    /// `ConfirmModal` wiring in its "loading" first paint, same as a real
    /// mount before the fetch lands.
    fn dialog_harness() -> Element {
        rsx! {
            DeleteBookDialog {
                uuid: "book-uuid".to_string(),
                title: "Piranesi".to_string(),
                on_deleted: move |_| {},
                on_close: move |_| {},
            }
        }
    }

    #[test]
    fn delete_book_dialog_renders_the_confirm_modal_shell_on_first_paint() {
        let html = crate::test_support::render_in_vdom(dialog_harness);
        assert!(html.contains("data-testid=\"delete-book-dialog\""));
        assert!(html.contains("author-photo-modal-backdrop"));
        assert!(html.contains("mg-modal del-modal"));
        assert!(html.contains("Loading this book\u{2019}s files\u{2026}"));
    }
}
