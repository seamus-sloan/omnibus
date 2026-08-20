//! Per-format CTA rows on the book detail page. Renders one row per format
//! the book has, sorted alphabetically, with per-format actions wired
//! underneath. When multiple files of the same format exist (after merge),
//! renders sub-rows with a file picker so the user can choose which file
//! to play or read.

mod async_action;
mod kindle;
mod kobo;

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;

use omnibus_shared::BookFileInfo;

#[cfg(not(feature = "mobile"))]
use crate::Route;

use kindle::send_to_kindle_action;
use kobo::send_to_kobo_action;

// The interactive send buttons only exist off-mobile (the mobile action helpers
// render disabled placeholders), so gate the re-exports to match — same as the
// parent `components/mod.rs` gates its re-exports of these.
#[cfg(not(feature = "mobile"))]
pub use kindle::SendToKindleButton;
#[cfg(not(feature = "mobile"))]
pub use kobo::SendToKoboButton;

/// The bottom-center send-result toast shared by
/// [`kindle::SendToKindleButton`] and [`kobo::SendToKoboButton`]: a status
/// message styled success/error plus a dismiss button. `prefix` ("kindle" /
/// "kobo") keys both the CSS class family and the testids.
#[cfg(not(feature = "mobile"))]
fn send_result_toast(prefix: &str, mut result: Signal<Option<(bool, String)>>) -> Element {
    let Some((is_error, message)) = result() else {
        return rsx! {};
    };
    rsx! {
        div { class: "{prefix}-toast card", role: "status",
            span {
                "data-testid": "{prefix}-send-status",
                class: if is_error { "{prefix}-toast-msg error" } else { "{prefix}-toast-msg success" },
                "{message}"
            }
            button {
                class: "btn ghost sm",
                "data-testid": "{prefix}-toast-dismiss",
                aria_label: "Dismiss",
                onclick: move |_| result.set(None),
                "\u{00d7}"
            }
        }
    }
}

/// Book-level metadata shared by the format/CTA action rows: identity for
/// links, author + title for the Send-to-Kobo `<Author>/<Title>/` layout,
/// the single-file EPUB size for the Send-to-Kindle email cap, and the
/// per-file rows for multi-file pickers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookActionMeta {
    pub uuid: String,
    pub author: String,
    pub title: String,
    pub epub_size_bytes: Option<i64>,
    pub book_files: Vec<BookFileInfo>,
}

/// Renders the format switcher, letting the reader pick which available
/// format of a book to open.
#[component]
pub fn FormatSwitcher(formats: Vec<String>, meta: BookActionMeta) -> Element {
    let BookActionMeta {
        uuid,
        author: book_author,
        title: book_title,
        epub_size_bytes,
        book_files,
    } = meta;
    let rows = prepare_rows(&formats);
    if rows.is_empty() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "format-switcher",
            role: "group",
            aria_label: "Available formats",
            "data-testid": "format-switcher",
            for row in rows {
                {
                    let files_for_format: Vec<&BookFileInfo> = book_files.iter()
                        .filter(|f| FormatKind::from_raw(&f.format) == row)
                        .collect();
                    if files_for_format.len() > 1 {
                        rsx! {
                            MultiFileRow {
                                key: "{row.label()}",
                                kind: row,
                                uuid: uuid.clone(),
                                files: files_for_format.into_iter().cloned().collect(),
                                book_author: book_author.clone(),
                                book_title: book_title.clone(),
                            }
                        }
                    } else {
                        rsx! {
                            FormatRow {
                                key: "{row.label()}",
                                kind: row,
                                uuid: uuid.clone(),
                                book_author: book_author.clone(),
                                book_title: book_title.clone(),
                                epub_size_bytes,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FormatRow(
    kind: FormatKind,
    uuid: String,
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
    #[props(default)] epub_size_bytes: Option<i64>,
) -> Element {
    let label = kind.label();
    let testid = format!("format-row-{}", label.to_ascii_lowercase());
    rsx! {
        div {
            class: "format-row",
            "data-format": "{label}",
            "data-testid": "{testid}",
            span { class: "format-badge", "data-testid": "format-badge", "{label}" }
            div { class: "format-actions",
                match kind {
                    FormatKind::Epub => rsx! {
                        // F2.2: web routes into the immersive reader; mobile
                        // stays disabled (no JS engine for epub.js — see F6.2).
                        // The cfg lives at the helper definition (rule 07:
                        // hydration parity — keep cfg gates out of rsx).
                        {read_book_action(&uuid)}
                        {send_to_kindle_action(&uuid, None, epub_size_bytes.unwrap_or_default())}
                        {send_to_kobo_action(&uuid, &book_author, &book_title)}
                    },
                    FormatKind::M4b | FormatKind::Mp3 => rsx! {
                        // F2.3: web routes into the immersive player; mobile
                        // stays disabled (no `<audio>` binding in the Dioxus
                        // Native shell yet — see F6.x).
                        {listen_book_action(&uuid)}
                    },
                    FormatKind::Other(_) => rsx! {
                        span { class: "format-actions-empty", "No actions yet" }
                    },
                }
            }
        }
    }
}

/// Expandable row for formats with multiple files (after merge). Shows the
/// format badge with a file count, and sub-rows for each file.
#[component]
fn MultiFileRow(
    kind: FormatKind,
    uuid: String,
    files: Vec<BookFileInfo>,
    #[props(default)] book_author: String,
    #[props(default)] book_title: String,
) -> Element {
    let label = kind.label();
    let testid = format!("format-row-{}", label.to_ascii_lowercase());
    let count = files.len();

    rsx! {
        div {
            class: "format-row format-row-multi",
            "data-format": "{label}",
            "data-testid": "{testid}",
            span { class: "format-badge", "data-testid": "format-badge",
                "{label} ({count} files)"
            }
            // Per-file Read/Listen live in the sub-rows below, but "Send to
            // Kobo" is book-level: the KEPUB endpoint converts the primary
            // (lowest-ordinal) EPUB, so a multi-EPUB book still gets the CTA
            // here. Per-file KEPUB is a deferred follow-up.
            if kind == FormatKind::Epub {
                div { class: "format-actions", {send_to_kobo_action(&uuid, &book_author, &book_title)} }
            }
        }
        for file in &files {
            {
                let file_label = file.label.clone()
                    .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
                let file_testid = format!("format-file-{}", file.id);
                let file_id = file.id;
                let file_size = file.size_bytes;
                rsx! {
                    div {
                        key: "{file_id}",
                        class: "format-row format-subrow",
                        "data-testid": "{file_testid}",
                        span { class: "format-sublabel", "{file_label}" }
                        div { class: "format-actions",
                            match kind {
                                // Per-file actions delegate to platform-gated
                                // helpers (rule 07: hydration parity — keep
                                // cfg gates out of rsx bodies).
                                FormatKind::Epub => rsx! {
                                    {read_file_action(&uuid, file_id)}
                                    {send_to_kindle_action(&uuid, Some(file_id), file_size)}
                                },
                                FormatKind::M4b | FormatKind::Mp3 => rsx! {
                                    {listen_file_action(&uuid, file_id)}
                                },
                                FormatKind::Other(_) => rsx! {
                                    span { class: "format-actions-empty", "No actions yet" }
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Per-target action helpers ────────────────────────────────────
//
// The cfg gate lives at the helper definition, not inside an rsx body.
// SSR (`feature = "server"`) and WASM (`feature = "web"`) both hit the
// `not(feature = "mobile")` arm and emit an identical `<a>` Link — so
// hydration parity holds (rule 07). Mobile (`feature = "mobile"`) is
// Dioxus Native, doesn't hydrate, and renders a disabled `<button>` as
// a placeholder until F6.x lands the native reader/player.

/// "Read" CTA for the book-level row. Web/SSR routes into the immersive
/// reader; mobile renders a disabled placeholder.
#[cfg(not(feature = "mobile"))]
fn read_book_action(uuid: &str) -> Element {
    rsx! {
        Link {
            to: Route::BookRead { uuid: uuid.to_string() },
            class: "btn",
            "data-testid": "action-read",
            "Read"
        }
    }
}

#[cfg(feature = "mobile")]
fn read_book_action(_uuid: &str) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Reading on mobile coming soon",
            "data-testid": "action-read",
            "Read"
        }
    }
}

/// "Listen" CTA for the book-level row.
#[cfg(not(feature = "mobile"))]
fn listen_book_action(uuid: &str) -> Element {
    rsx! {
        Link {
            to: Route::BookListen { uuid: uuid.to_string(), file_id: None },
            class: "btn",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

#[cfg(feature = "mobile")]
fn listen_book_action(_uuid: &str) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Listening on mobile coming soon",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

/// Per-file "Read" CTA used inside a `MultiFileRow`. Routes carry a
/// `file_id` query so the reader opens the chosen file.
#[cfg(not(feature = "mobile"))]
fn read_file_action(uuid: &str, file_id: i64) -> Element {
    let href = format!("/read/{uuid}?file_id={file_id}");
    rsx! {
        Link {
            to: "{href}",
            class: "btn",
            "data-testid": "action-read",
            "Read"
        }
    }
}

#[cfg(feature = "mobile")]
fn read_file_action(_uuid: &str, _file_id: i64) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            "data-testid": "action-read",
            "Read"
        }
    }
}

/// Per-file "Listen" CTA used inside a `MultiFileRow`.
#[cfg(not(feature = "mobile"))]
fn listen_file_action(uuid: &str, file_id: i64) -> Element {
    let href = format!("/listen/{uuid}?file_id={file_id}");
    rsx! {
        Link {
            to: "{href}",
            class: "btn",
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

#[cfg(feature = "mobile")]
fn listen_file_action(_uuid: &str, _file_id: i64) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            "data-testid": "action-listen",
            "Listen"
        }
    }
}

/// One row in the switcher. `Other(String)` keeps the original casing of the
/// raw `book_files.format` value so the badge displays whatever the schema
/// stored (e.g. "PDF", "CBZ") without invoking a giant match.
#[derive(Clone, PartialEq, Eq)]
enum FormatKind {
    Epub,
    M4b,
    Mp3,
    Other(String),
}

impl FormatKind {
    fn from_raw(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("EPUB") {
            FormatKind::Epub
        } else if raw.eq_ignore_ascii_case("M4B") || raw.eq_ignore_ascii_case("M4A") {
            // M4A is the same MPEG-4 container as M4B with a different
            // extension; both flow through the F2.3 player and the
            // tower-http serve handler under the same code path.
            FormatKind::M4b
        } else if raw.eq_ignore_ascii_case("MP3") {
            FormatKind::Mp3
        } else {
            FormatKind::Other(raw.to_string())
        }
    }

    fn label(&self) -> &str {
        match self {
            FormatKind::Epub => "EPUB",
            FormatKind::M4b => "M4B",
            FormatKind::Mp3 => "MP3",
            FormatKind::Other(s) => s.as_str(),
        }
    }
}

/// Dedupe (case-insensitive), sort alphabetical by label (also case-
/// insensitive — otherwise unknown-cased rows like `"cbz"` would sort after
/// the upper-cased known ones, which doesn't match the "alphabetical"
/// contract or the dedupe logic), and map raw format strings to the typed
/// rows the switcher renders.
fn prepare_rows(formats: &[String]) -> Vec<FormatKind> {
    let mut rows: Vec<FormatKind> = formats.iter().map(|s| FormatKind::from_raw(s)).collect();
    rows.sort_by(|a, b| {
        a.label()
            .to_ascii_lowercase()
            .cmp(&b.label().to_ascii_lowercase())
    });
    rows.dedup_by(|a, b| a.label().eq_ignore_ascii_case(b.label()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_rows_sorts_alphabetical() {
        let rows = prepare_rows(&["M4B".into(), "EPUB".into(), "PDF".into()]);
        assert_eq!(
            rows.iter()
                .map(super::FormatKind::label)
                .collect::<Vec<_>>(),
            vec!["EPUB", "M4B", "PDF"],
        );
    }

    #[test]
    fn prepare_rows_dedupes_case_insensitively() {
        let rows = prepare_rows(&["epub".into(), "EPUB".into()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label(), "EPUB");
    }

    #[test]
    fn prepare_rows_sorts_case_insensitively() {
        // Regression for PR #65 review: mixed-case input must produce a
        // consistent alphabetical order regardless of casing — otherwise
        // upper-cased known formats (EPUB, M4B) would always sort before
        // lower-cased unknown ones (cbz), which surprises users.
        let rows = prepare_rows(&["PDF".into(), "cbz".into(), "EPUB".into()]);
        assert_eq!(
            rows.iter()
                .map(super::FormatKind::label)
                .collect::<Vec<_>>(),
            vec!["cbz", "EPUB", "PDF"],
        );
    }

    #[test]
    fn prepare_rows_preserves_unknown_format_casing() {
        let rows = prepare_rows(&["CbZ".into()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label(), "CbZ");
        assert!(matches!(rows[0], FormatKind::Other(_)));
    }

    #[test]
    fn empty_input_renders_nothing_meaningful() {
        // We don't exercise the actual rsx! macro (no SSR dep in this crate),
        // but the prepare_rows path is what gates the FormatSwitcher's
        // `rows.is_empty() → return rsx!{}` branch.
        assert!(prepare_rows(&[]).is_empty());
    }
}
