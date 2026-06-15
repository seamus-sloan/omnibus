//! Per-format CTA rows on the book detail page. Renders one row per format
//! the book has, sorted alphabetically, with per-format actions wired
//! underneath. When multiple files of the same format exist (after merge),
//! renders sub-rows with a file picker so the user can choose which file
//! to play or read.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;

use omnibus_shared::BookFileInfo;

#[cfg(not(feature = "mobile"))]
use crate::Route;

#[component]
pub fn FormatSwitcher(
    formats: Vec<String>,
    uuid: String,
    #[props(default)] book_files: Vec<BookFileInfo>,
) -> Element {
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
                            }
                        }
                    } else {
                        rsx! {
                            FormatRow { key: "{row.label()}", kind: row, uuid: uuid.clone() }
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
    #[cfg_attr(feature = "mobile", allow(unused_variables))] uuid: String,
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
                        {
                            // F2.2: web routes into the immersive reader; mobile
                            // stays disabled (no JS engine for epub.js — see F6.2).
                            #[cfg(not(feature = "mobile"))]
                            let read_action = rsx! {
                                Link {
                                    to: Route::BookRead { uuid: uuid.clone() },
                                    class: "btn",
                                    "data-testid": "action-read",
                                    "Read"
                                }
                            };
                            #[cfg(feature = "mobile")]
                            let read_action = rsx! {
                                button {
                                    class: "btn",
                                    disabled: true,
                                    title: "Reading on mobile coming soon",
                                    "data-testid": "action-read",
                                    "Read"
                                }
                            };
                            read_action
                        }
                        button {
                            class: "btn",
                            disabled: true,
                            title: "Send-to-Kindle coming soon",
                            "data-testid": "action-kindle",
                            "Send to Kindle"
                        }
                    },
                    FormatKind::M4b | FormatKind::Mp3 => rsx! {
                        {
                            // F2.3: web routes into the immersive player; mobile
                            // stays disabled (no `<audio>` binding in the Dioxus
                            // Native shell yet — see F6.x).
                            #[cfg(not(feature = "mobile"))]
                            let listen_action = rsx! {
                                Link {
                                    to: Route::BookListen { uuid: uuid.clone() },
                                    class: "btn",
                                    "data-testid": "action-listen",
                                    "Listen"
                                }
                            };
                            #[cfg(feature = "mobile")]
                            let listen_action = rsx! {
                                button {
                                    class: "btn",
                                    disabled: true,
                                    title: "Listening on mobile coming soon",
                                    "data-testid": "action-listen",
                                    "Listen"
                                }
                            };
                            listen_action
                        }
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
    #[cfg_attr(feature = "mobile", allow(unused_variables))] uuid: String,
    files: Vec<BookFileInfo>,
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
        }
        for file in &files {
            {
                let file_label = file.label.clone()
                    .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
                let file_testid = format!("format-file-{}", file.id);
                rsx! {
                    div {
                        class: "format-row format-subrow",
                        "data-testid": "{file_testid}",
                        span { class: "format-sublabel", "{file_label}" }
                        div { class: "format-actions",
                            match kind {
                                FormatKind::Epub => rsx! {
                                    {
                                        #[cfg(not(feature = "mobile"))]
                                        let read_action = {
                                            let href = format!("/read/{uuid}?file_id={}", file.id);
                                            rsx! {
                                                Link {
                                                    to: "{href}",
                                                    class: "btn",
                                                    "data-testid": "action-read",
                                                    "Read"
                                                }
                                            }
                                        };
                                        #[cfg(feature = "mobile")]
                                        let read_action = rsx! {
                                            button {
                                                class: "btn",
                                                disabled: true,
                                                "data-testid": "action-read",
                                                "Read"
                                            }
                                        };
                                        read_action
                                    }
                                },
                                FormatKind::M4b | FormatKind::Mp3 => rsx! {
                                    {
                                        #[cfg(not(feature = "mobile"))]
                                        let listen_action = {
                                            let href = format!("/listen/{uuid}?file_id={}", file.id);
                                            rsx! {
                                                Link {
                                                    to: "{href}",
                                                    class: "btn",
                                                    "data-testid": "action-listen",
                                                    "Listen"
                                                }
                                            }
                                        };
                                        #[cfg(feature = "mobile")]
                                        let listen_action = rsx! {
                                            button {
                                                class: "btn",
                                                disabled: true,
                                                "data-testid": "action-listen",
                                                "Listen"
                                            }
                                        };
                                        listen_action
                                    }
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
