//! File picker dropdown for the hero/mobile Read + Listen CTAs — lets the
//! reader choose which physical `book_files` row to open when a book has
//! more than one file of the format the CTA opens. Mirrors `export_menu`'s
//! trigger + scrim + focus-on-mount dialog pattern; shared between the web
//! hero and the mobile CTA row.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::BookFileInfo;

use crate::focus_after_paint::focus_after_paint;

/// Which action the picker's rows perform — drives the route prefix, the
/// dialog's accessible label, and the stable testids.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FilePickerKind {
    Read,
    Listen,
}

impl FilePickerKind {
    fn route_prefix(self) -> &'static str {
        match self {
            FilePickerKind::Read => "read",
            FilePickerKind::Listen => "listen",
        }
    }

    fn testid(self) -> &'static str {
        match self {
            FilePickerKind::Read => "read-file-picker",
            FilePickerKind::Listen => "listen-file-picker",
        }
    }

    fn aria_label(self) -> &'static str {
        match self {
            FilePickerKind::Read => "Choose which file to read",
            FilePickerKind::Listen => "Choose which file to listen to",
        }
    }
}

/// True for the audio `book_files.format` values the listen path resolves
/// (mirrors `db::hls::resolve_audiobook_file`'s `format IN ('M4B', 'M4A',
/// 'MP3')` filter, since M4A shares the M4B container/code path).
pub(super) fn is_audio_book_file(f: &BookFileInfo) -> bool {
    f.format.eq_ignore_ascii_case("M4B")
        || f.format.eq_ignore_ascii_case("M4A")
        || f.format.eq_ignore_ascii_case("MP3")
}

fn file_picker_item_title(file: &BookFileInfo, kind: FilePickerKind) -> String {
    let format = match kind {
        FilePickerKind::Read => file.format.to_uppercase(),
        FilePickerKind::Listen => "Audiobook".to_string(),
    };
    let label = file
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
    format!("{format} \u{b7} {label}")
}

fn file_picker_item_meta(file: &BookFileInfo, kind: FilePickerKind) -> Option<String> {
    let size = crate::format::file_size(file.size_bytes)?;
    let label = file
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty());
    match (kind, label) {
        (FilePickerKind::Listen, Some(label)) => Some(format!("{label} \u{b7} {size}")),
        _ => Some(size),
    }
}

/// Read/listen action that expands into a file picker when multiple files match.
#[component]
pub(super) fn BdFilePickerMenu(
    uuid: String,
    kind: FilePickerKind,
    files: Vec<BookFileInfo>,
    label: String,
    button_class: String,
    single_testid: String,
) -> Element {
    let prefix = kind.route_prefix();
    if files.len() < 2 {
        let href = format!("/{prefix}/{uuid}");
        return rsx! {
            Link {
                to: "{href}",
                class: "{button_class}",
                "data-testid": "{single_testid}",
                "{label}"
            }
        };
    }
    let mut open = use_signal(|| false);
    let testid = kind.testid();
    let trigger_class = format!("{button_class} bd-file-picker-trigger");
    rsx! {
        div { class: "bd-file-picker",
            button {
                class: "{trigger_class}",
                "data-testid": "{testid}-trigger",
                r#type: "button",
                "aria-haspopup": "dialog",
                "aria-expanded": "{open()}",
                "aria-label": kind.aria_label(),
                onclick: move |_| {
                    let next = !open();
                    open.set(next);
                },
                "{label}"
                span { class: "bd-file-picker-caret", aria_hidden: "true", "\u{25be}" }
            }
            if open() {
                div {
                    class: "bd-export-scrim",
                    "data-testid": "{testid}-scrim",
                    onclick: move |_| open.set(false),
                }
                BdFilePickerPanel { uuid: uuid.clone(), kind, files: files.clone(), open }
            }
        }
    }
}

/// The open dropdown body. Split out (same reason as `BdExportPanel`) so
/// `onmounted` can focus it and ESC reaches the panel-level `onkeydown`.
#[component]
fn BdFilePickerPanel(
    uuid: String,
    kind: FilePickerKind,
    files: Vec<BookFileInfo>,
    open: Signal<bool>,
) -> Element {
    let mut open = open;
    let on_keydown = move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Escape {
            evt.prevent_default();
            open.set(false);
        }
    };
    let testid = kind.testid();
    let prefix = kind.route_prefix();
    let heading_testid = format!("{testid}-heading");
    rsx! {
        div {
            class: "bd-export-panel bd-file-picker-panel card",
            role: "dialog",
            "aria-label": kind.aria_label(),
            "data-testid": "{testid}-panel",
            tabindex: "-1",
            onkeydown: on_keydown,
            onmounted: move |evt: MountedEvent| focus_after_paint(&evt),

            div {
                class: "label bd-file-picker-heading",
                "data-testid": "{heading_testid}",
                "{files.len()} files \u{b7} choose one"
            }
            for file in files.iter() {
                {
                    let title = file_picker_item_title(file, kind);
                    let meta = file_picker_item_meta(file, kind);
                    let href = format!("/{prefix}/{uuid}?file_id={}", file.id);
                    let item_testid = format!("{testid}-item-{}", file.id);
                    rsx! {
                        Link {
                            key: "{file.id}",
                            to: "{href}",
                            class: "bd-export-item bd-file-picker-item",
                            "data-testid": "{item_testid}",
                            onclick: move |_| open.set(false),
                            div { class: "bd-file-picker-item-copy",
                                div { class: "bd-file-picker-item-title", "{title}" }
                                if let Some(meta) = meta.as_deref() {
                                    div { class: "mono bd-file-picker-item-meta", "{meta}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(format: &str, id: i64, ordinal: i64) -> BookFileInfo {
        BookFileInfo {
            id,
            format: format.to_string(),
            filename: format!("part-{ordinal}.{}", format.to_lowercase()),
            ordinal,
            label: None,
            size_bytes: 0,
            path: None,
            etag: None,
        }
    }

    #[test]
    fn is_audio_book_file_matches_every_hls_audio_format_case_insensitively() {
        for fmt in ["M4B", "m4b", "M4A", "m4a", "MP3", "mp3"] {
            assert!(is_audio_book_file(&file(fmt, 1, 0)));
        }
    }

    #[test]
    fn is_audio_book_file_rejects_non_audio_formats() {
        assert!(!is_audio_book_file(&file("EPUB", 1, 0)));
        assert!(!is_audio_book_file(&file("PDF", 1, 0)));
    }

    #[test]
    fn file_picker_item_title_matches_the_read_and_listen_design_labels() {
        let mut epub = file("EPUB", 1, 0);
        epub.label = Some("10th anniversary".to_string());
        let mut audiobook = file("M4B", 2, 1);
        audiobook.label = Some("Full cast".to_string());

        assert_eq!(
            file_picker_item_title(&epub, FilePickerKind::Read),
            "EPUB · 10th anniversary"
        );
        assert_eq!(
            file_picker_item_title(&audiobook, FilePickerKind::Listen),
            "Audiobook · Full cast"
        );
    }

    #[test]
    fn file_picker_item_meta_formats_file_size_and_audio_label() {
        let mut epub = file("EPUB", 1, 0);
        epub.size_bytes = 3_100_000;
        let mut audiobook = file("M4B", 2, 1);
        audiobook.label = Some("Full cast".to_string());
        audiobook.size_bytes = 512_000_000;

        assert_eq!(
            file_picker_item_meta(&epub, FilePickerKind::Read),
            Some("3.1 MB".to_string())
        );
        assert_eq!(
            file_picker_item_meta(&audiobook, FilePickerKind::Listen),
            Some("Full cast · 512.0 MB".to_string())
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn file_picker_menu_integrates_the_caret_into_the_action_for_multiple_files() {
        let html = dioxus::ssr::render_element(rsx! {
            BdFilePickerMenu {
                uuid: "book-a".to_string(),
                kind: FilePickerKind::Listen,
                files: vec![file("MP3", 1, 0), file("MP3", 2, 1)],
                label: "Start listening".to_string(),
                button_class: "btn primary lg".to_string(),
                single_testid: "start-listening".to_string(),
            }
        });

        assert!(html.contains("data-testid=\"listen-file-picker-trigger\""));
        assert!(html.contains("Start listening"));
        assert!(html.contains("\u{25be}"));
        assert!(!html.contains("data-testid=\"start-listening\""));
    }

    #[test]
    fn file_picker_kind_read_uses_the_read_route_prefix_and_testid() {
        assert_eq!(FilePickerKind::Read.route_prefix(), "read");
        assert_eq!(FilePickerKind::Read.testid(), "read-file-picker");
    }

    #[test]
    fn file_picker_kind_listen_uses_the_listen_route_prefix_and_testid() {
        assert_eq!(FilePickerKind::Listen.route_prefix(), "listen");
        assert_eq!(FilePickerKind::Listen.testid(), "listen-file-picker");
    }
}
