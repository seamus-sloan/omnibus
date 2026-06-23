//! Per-cell components for the landing table's inline-editable rows.
//!
//! Each helper here wraps an [`EditableCell`] (or, for authors, hosts a
//! [`ChipEditor`] directly) and is composed by `EbookRowCells` in
//! [`super::row`]. The two `build_save_*` helpers manufacture the
//! `on_save` callbacks the row passes down into the cells.

use dioxus::prelude::*;
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use super::super::filtering::format_badge_label;
use super::EditField;
use crate::components::chip_editor::{ChipEditor, SuggestionItem};
use crate::data;

/// Build the common per-cell save callback. Empty strings clear the
/// override (None — the scanned value re-surfaces); non-empty strings
/// persist as `Some(value)`. The server-side merge in `rpc_save_overrides`
/// returns the canonical merged metadata which we install verbatim into
/// the optimistic `book_state` signal.
pub(super) fn build_save_field(
    uuid: String,
    server_url: String,
    mut book_state: Signal<EbookMetadata>,
) -> impl FnMut((EditField, String)) + 'static {
    move |args: (EditField, String)| {
        let (field, value) = args;
        let uuid = uuid.clone();
        let url = server_url.clone();
        spawn(async move {
            let mut overrides = MetadataOverrides::default();
            let trimmed = value.trim();
            let value = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            match field {
                EditField::Title => overrides.title = value,
                EditField::Series => overrides.series = value,
                EditField::Publisher => overrides.publisher = value,
                EditField::Published => overrides.published = value,
                EditField::Language => overrides.language = value,
                // Authors edits go through `build_save_authors` so they
                // can set the `creators` override instead of a scalar.
                EditField::Authors => return,
            }
            if overrides.validate().is_err() {
                // F5.1 length caps rejected the payload. The display
                // reverts on the next refetch; a toast / inline error
                // message ships in a follow-up PR.
                return;
            }
            if let Ok(Some(merged)) = data::save_overrides(&url, &uuid, &overrides).await {
                book_state.set(merged);
            }
        });
    }
}

/// Build the authors-cell save callback. Mirrors [`build_save_field`] but
/// posts a `creators` override (full replacement list) instead of a scalar.
pub(super) fn build_save_authors(
    uuid: String,
    server_url: String,
    mut book_state: Signal<EbookMetadata>,
) -> impl FnMut(Vec<String>) + 'static {
    move |new_names: Vec<String>| {
        let uuid = uuid.clone();
        let url = server_url.clone();
        spawn(async move {
            let creators: Vec<Contributor> = new_names
                .iter()
                .map(|name| Contributor {
                    name: name.clone(),
                    ..Default::default()
                })
                .collect();
            let overrides = MetadataOverrides {
                creators: Some(creators),
                ..Default::default()
            };
            if overrides.validate().is_err() {
                return;
            }
            if let Ok(Some(merged)) = data::save_overrides(&url, &uuid, &overrides).await {
                book_state.set(merged);
            }
        });
    }
}

/// Title cell wrapper — pre-wires the title field's [`EditableCell`] props
/// and routes its `on_save` through the shared row save callback.
#[component]
pub(super) fn RowTitleCell(
    title: String,
    error: Option<String>,
    is_admin: bool,
    editing: Signal<Option<EditField>>,
    save_field: EventHandler<(EditField, String)>,
) -> Element {
    rsx! {
        EditableCell {
            col_class: "ebook-col-title".to_string(),
            cell_testid: "ebook-cell-title".to_string(),
            field: EditField::Title,
            display_value: title,
            is_admin,
            editing,
            placeholder: "Title".to_string(),
            on_save: move |v: String| save_field.call((EditField::Title, v)),
            error,
        }
    }
}

/// Series cell wrapper — display shows "Name #Index" (F1.3), edit input
/// seeds with just the series name (the index lives on the full edit page).
#[component]
pub(super) fn RowSeriesCell(
    series_line: String,
    series_text: String,
    is_admin: bool,
    editing: Signal<Option<EditField>>,
    save_field: EventHandler<(EditField, String)>,
) -> Element {
    rsx! {
        EditableCell {
            col_class: "ebook-col-series".to_string(),
            cell_testid: "ebook-cell-series".to_string(),
            field: EditField::Series,
            display_value: series_line,
            edit_value: Some(series_text),
            is_admin,
            editing,
            placeholder: "Series".to_string(),
            on_save: move |v: String| save_field.call((EditField::Series, v)),
            error: None,
        }
    }
}

/// Plain scalar-text cell wrapper for publisher / published / language —
/// collapses the boilerplate of wiring `EditableCell` to the shared
/// `save_field` callback.
#[component]
pub(super) fn RowScalarCell(
    col_class: String,
    cell_testid: String,
    field: EditField,
    value: String,
    placeholder: String,
    is_admin: bool,
    editing: Signal<Option<EditField>>,
    save_field: EventHandler<(EditField, String)>,
) -> Element {
    rsx! {
        EditableCell {
            col_class,
            cell_testid,
            field,
            display_value: value,
            is_admin,
            editing,
            placeholder,
            on_save: move |v: String| save_field.call((field, v)),
            error: None,
        }
    }
}

/// Cover-thumbnail `<td>` — image with srcset when a cover exists, em-dash
/// fallback otherwise.
#[component]
pub(super) fn EbookRowCoverCell(thumb_base: String, has_cover: bool, alt_title: String) -> Element {
    rsx! {
        td { class: "ebook-col-cover", "data-testid": "ebook-cell-cover",
            if has_cover {
                img {
                    class: "ebook-thumb",
                    src: "{thumb_base}/md",
                    srcset: "{thumb_base}/sm 160w, {thumb_base}/md 320w, {thumb_base}/lg 640w",
                    sizes: "(max-width: 640px) 160px, (max-width: 1280px) 320px, 640px",
                    alt: "Cover of {alt_title}",
                    loading: "lazy",
                    width: "320",
                    height: "480",
                }
            } else {
                div { class: "ebook-thumb ebook-thumb-fallback", "—" }
            }
        }
    }
}

/// Formats `<td>` — one badge span per format, em-dash when empty.
#[component]
pub(super) fn EbookRowFormatsCell(formats: Vec<String>) -> Element {
    rsx! {
        td { class: "ebook-col-formats", "data-testid": "ebook-cell-formats",
            if formats.is_empty() {
                span { class: "ebook-cell-formats-empty", "—" }
            } else {
                for fmt in formats.iter() {
                    span { key: "{fmt}", class: "format-badge", "{format_badge_label(fmt)}" }
                }
            }
        }
    }
}

/// Inline-editable text cell used by `EbookRow`. Renders a span of
/// text by default; in admin mode, a click swaps to a text input that
/// commits via `EditField::on_save` on Enter or blur and cancels on
/// Escape. The input's `onclick` stops propagation so the row-level
/// navigate handler doesn't fire while the user is editing.
///
/// `suppress_inline_input` is the Authors-cell escape hatch: the click
/// still toggles `editing` (so the expansion row appears) but the
/// in-cell input never renders.
#[component]
pub(super) fn EditableCell(
    col_class: String,
    cell_testid: String,
    field: EditField,
    display_value: String,
    /// Separate value used to seed the input when edit mode opens.
    /// Some cells render a richer display string than the underlying
    /// scalar override — Series shows "Pioneers #1" but only "Pioneers"
    /// is the series-name override. Pass `Some(bare_value)` so the
    /// input seeds (and the blur-comparison runs against) the editable
    /// scalar, not the rendered text. Defaults to `display_value`.
    #[props(default)]
    edit_value: Option<String>,
    is_admin: bool,
    editing: Signal<Option<EditField>>,
    placeholder: String,
    on_save: EventHandler<String>,
    error: Option<String>,
) -> Element {
    let mut editing = editing;
    let mut draft = use_signal(String::new);
    let is_editing = editing() == Some(field);
    let active_class = if is_editing {
        " ebook-cell-editing"
    } else {
        ""
    };
    let admin_class = if is_admin { " ebook-cell-editable" } else { "" };
    let combined_class = format!("{col_class}{admin_class}{active_class}");
    // Single source of truth for "what the cell holds right now" —
    // used to seed the draft on click, restore it on Escape, and skip
    // the save-on-blur round-trip when nothing actually changed.
    let initial = edit_value.unwrap_or_else(|| display_value.clone());
    let initial_for_click = initial.clone();
    let initial_for_cancel = initial.clone();
    let initial_for_enter = initial.clone();
    let initial_for_blur = initial.clone();
    let testid_input = format!("{cell_testid}-input");

    rsx! {
        td {
            class: "{combined_class}",
            "data-testid": "{cell_testid}",
            onclick: move |e| {
                if !is_admin {
                    return;
                }
                e.stop_propagation();
                if !is_editing {
                    draft.set(initial_for_click.clone());
                    editing.set(Some(field));
                }
            },
            if is_editing {
                input {
                    class: "ebook-cell-edit",
                    "data-testid": "{testid_input}",
                    autofocus: true,
                    value: "{draft}",
                    placeholder: "{placeholder}",
                    onclick: move |e| { e.stop_propagation(); },
                    oninput: move |e| { draft.set(e.value()); },
                    onkeydown: move |e: Event<KeyboardData>| {
                        match e.key() {
                            Key::Enter => {
                                e.prevent_default();
                                // Stop the Enter event from bubbling to
                                // the row-level keydown that navigates
                                // to the book detail page.
                                e.stop_propagation();
                                let current = draft();
                                if current.trim() != initial_for_enter.trim() {
                                    on_save.call(current);
                                }
                                editing.set(None);
                            }
                            Key::Escape => {
                                e.prevent_default();
                                e.stop_propagation();
                                draft.set(initial_for_cancel.clone());
                                editing.set(None);
                            }
                            _ => {}
                        }
                    },
                    onblur: move |_| {
                        // Save on blur only when the draft actually
                        // changed. Clicking into a cell and clicking
                        // back out without typing must not POST an
                        // override equal to the scanned value (which
                        // would leak `metadata_overrides` rows that
                        // match the underlying scan, defeating the
                        // F5.1 merge semantics).
                        let current = draft();
                        if current.trim() != initial_for_blur.trim() {
                            on_save.call(current);
                        }
                        editing.set(None);
                    },
                }
            } else {
                div { class: "ebook-title-cell", "{display_value}" }
                if let Some(err) = error.as_ref() {
                    div { class: "error", "⚠ {err}" }
                }
            }
        }
    }
}

/// Inline-editable Authors cell. Unlike [`EditableCell`] (which hosts
/// a single-line text input), the Authors cell renders the full
/// [`ChipEditor`] *inside* the `<td>` when the row's editing signal
/// matches `EditField::Authors`. The cell grows vertically to fit the
/// chips + input + dropdown, matching the design comp.
///
/// Display mode: comma-joined names. Hover: dashed amber outline.
/// Click: swaps to chip editor (auto-focused) with the library-wide
/// author pool surfaced in the dropdown. Escape exits edit mode.
#[component]
pub(super) fn AuthorsCell(
    is_admin: bool,
    editing: Signal<Option<EditField>>,
    authors_draft: Signal<Vec<String>>,
    authors_text: String,
    suggestions: ReadSignal<Vec<SuggestionItem>>,
    on_change: EventHandler<Vec<String>>,
) -> Element {
    let mut editing = editing;
    let is_editing = editing() == Some(EditField::Authors);
    let active_class = if is_editing {
        " ebook-cell-editing"
    } else {
        ""
    };
    let admin_class = if is_admin { " ebook-cell-editable" } else { "" };
    let combined_class = format!("ebook-col-author{admin_class}{active_class}");

    rsx! {
        td {
            class: "{combined_class}",
            "data-testid": "ebook-cell-author",
            onclick: move |e| {
                if !is_admin {
                    return;
                }
                e.stop_propagation();
                if !is_editing {
                    editing.set(Some(EditField::Authors));
                }
            },
            if is_editing {
                div { class: "ebook-cell-chip-host",
                    ChipEditor {
                        values: authors_draft,
                        placeholder: "+ add author\u{2026}".to_string(),
                        on_change,
                        suggestions,
                        show_avatar: false,
                        aria_remove_prefix: "Remove".to_string(),
                        testid_prefix: "ebook-cell-author".to_string(),
                        autofocus: true,
                        dropdown_header: "ADD AUTHOR".to_string(),
                        on_close: move |_| {
                            editing.set(None);
                        },
                    }
                }
            } else {
                div { class: "ebook-title-cell", "{authors_text}" }
            }
        }
    }
}
