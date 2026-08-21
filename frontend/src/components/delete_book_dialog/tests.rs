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
        etag: None,
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
