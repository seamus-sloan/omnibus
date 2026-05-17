//! Per-format CTAs on the book detail page (F1.4).
//!
//! Renders one row per format the book has, sorted alphabetically, with the
//! relevant actions wired underneath. All actions stay disabled in Phase 1:
//! Read (F2.2 reader), Listen (F2.3 player), and Send to Kindle (F4.x) ship
//! later. The rows themselves are the UI contract for the `books` /
//! `book_files` split from F0.1 — a work with both EPUB and M4B surfaces both
//! formats here so the future per-format actions slot in without re-shaping
//! the markup.

use dioxus::prelude::*;

#[component]
pub fn FormatSwitcher(formats: Vec<String>) -> Element {
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
                FormatRow { kind: row }
            }
        }
    }
}

#[component]
fn FormatRow(kind: FormatKind) -> Element {
    let label = kind.label();
    rsx! {
        div {
            class: "format-row",
            "data-format": "{label}",
            span { class: "format-badge", "{label}" }
            div { class: "format-actions",
                match kind {
                    FormatKind::Epub => rsx! {
                        button {
                            class: "btn",
                            disabled: true,
                            title: "Reader coming soon",
                            "data-testid": "action-read",
                            "Read"
                        }
                        button {
                            class: "btn",
                            disabled: true,
                            title: "Send-to-Kindle coming soon",
                            "data-testid": "action-kindle",
                            "Send to Kindle"
                        }
                    },
                    FormatKind::M4b => rsx! {
                        button {
                            class: "btn",
                            disabled: true,
                            title: "Audio player coming soon",
                            "data-testid": "action-listen",
                            "Listen"
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

/// One row in the switcher. `Other(String)` keeps the original casing of the
/// raw `book_files.format` value so the badge displays whatever the schema
/// stored (e.g. "PDF", "CBZ") without invoking a giant match.
#[derive(Clone, PartialEq, Eq)]
enum FormatKind {
    Epub,
    M4b,
    Other(String),
}

impl FormatKind {
    fn from_raw(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("EPUB") {
            FormatKind::Epub
        } else if raw.eq_ignore_ascii_case("M4B") {
            FormatKind::M4b
        } else {
            FormatKind::Other(raw.to_string())
        }
    }

    fn label(&self) -> &str {
        match self {
            FormatKind::Epub => "EPUB",
            FormatKind::M4b => "M4B",
            FormatKind::Other(s) => s.as_str(),
        }
    }
}

/// Dedupe (case-insensitive), sort alphabetical by label, and map raw format
/// strings to the typed rows the switcher renders.
fn prepare_rows(formats: &[String]) -> Vec<FormatKind> {
    let mut rows: Vec<FormatKind> = formats.iter().map(|s| FormatKind::from_raw(s)).collect();
    rows.sort_by(|a, b| a.label().cmp(b.label()));
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
            rows.iter().map(|r| r.label()).collect::<Vec<_>>(),
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
