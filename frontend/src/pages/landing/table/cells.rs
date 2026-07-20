//! Per-cell components for the landing table's inline-editable rows.
//!
//! Each helper wraps an [`EditableCell`] (or, for authors, hosts a [`ChipEditor`]
//! directly) and is composed by `EbookRowCells` in [`super::row`].
//! `build_save_*` produce the row's per-field save callbacks.

use dioxus::prelude::*;
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use super::super::filtering::format_badge_label;
use super::EditField;
use crate::components::chip_editor::{ChipEditor, ChipEditorOptions, SuggestionItem};
use crate::data;

/// Shared edit context for a scalar cell wrapper: admin gating, the row's
/// current editing signal, and the per-field save callback. Grouped so the
/// title/series/scalar wrappers stay under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct CellEditCtx {
    pub is_admin: bool,
    pub editing: Signal<Option<EditField>>,
    pub save_field: EventHandler<(EditField, String)>,
}

/// Per-row context threaded from `EbookRow` through `EbookRowMarkup` into
/// `EbookRowCells`: admin gating, editing state, the authors chip draft +
/// suggestion pool, and the two save callbacks. Grouped so the row markup
/// components stay under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct RowContext {
    pub is_admin: bool,
    pub editing: Signal<Option<EditField>>,
    pub authors_draft: Signal<Vec<String>>,
    pub author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub save_field: EventHandler<(EditField, String)>,
    pub save_authors: EventHandler<Vec<String>>,
}

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
pub(super) fn RowTitleCell(title: String, error: Option<String>, ctx: CellEditCtx) -> Element {
    let CellEditCtx {
        is_admin,
        editing,
        save_field,
    } = ctx;
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

/// Series cell wrapper — display shows "Name #Index", edit input
/// seeds with just the series name (the index lives on the full edit page).
#[component]
pub(super) fn RowSeriesCell(series_line: String, series_text: String, ctx: CellEditCtx) -> Element {
    let CellEditCtx {
        is_admin,
        editing,
        save_field,
    } = ctx;
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
    ctx: CellEditCtx,
) -> Element {
    let CellEditCtx {
        is_admin,
        editing,
        save_field,
    } = ctx;
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
pub(super) fn EbookRowCoverCell(
    thumb_src: String,
    thumb_srcset: String,
    has_cover: bool,
    alt_title: String,
) -> Element {
    // A transient thumb-fetch failure otherwise renders the browser's
    // broken-image icon with no self-heal until a full reload. Tracks the
    // *url* that failed (not just a bool) so a later render with a
    // different url — a mobile token rotation, or (once #832 item 4
    // versions thumb URLs) a regenerated thumb — always gets a fresh
    // chance to load, even though this row's component instance stays
    // mounted (same key) across the change.
    let mut broken_thumb_src: Signal<Option<String>> = use_signal(|| None);
    let show_img = has_cover && broken_thumb_src.read().as_deref() != Some(thumb_src.as_str());
    rsx! {
        td { class: "ebook-col-cover", "data-testid": "ebook-cell-cover",
            if show_img {
                img {
                    class: "ebook-thumb",
                    src: "{thumb_src}",
                    srcset: "{thumb_srcset}",
                    sizes: "(max-width: 640px) 160px, (max-width: 1280px) 320px, 640px",
                    alt: "Cover of {alt_title}",
                    loading: "lazy",
                    width: "320",
                    height: "480",
                    onerror: {
                        let thumb_src = thumb_src.clone();
                        move |_| broken_thumb_src.set(Some(thumb_src.clone()))
                    },
                }
            } else {
                div { class: "ebook-thumb ebook-thumb-fallback", "—" }
            }
        }
    }
}

/// Formats `<td>` — one badge span per format, em-dash when empty.
#[component]
pub(super) fn EbookRowFormatsCell(formats: Vec<String>, has_physical: bool) -> Element {
    rsx! {
        td { class: "ebook-col-formats", "data-testid": "ebook-cell-formats",
            if formats.is_empty() && !has_physical {
                span { class: "ebook-cell-formats-empty", "—" }
            } else {
                for fmt in formats.iter() {
                    span { key: "{fmt}", class: "format-badge", "{format_badge_label(fmt)}" }
                }
                if has_physical {
                    span {
                        class: "format-badge format-badge--physical",
                        "data-testid": "format-badge-physical",
                        title: "In your physical collection",
                        "PHYS"
                    }
                }
            }
        }
    }
}

/// Props for the [`EditableCell`] component.
#[derive(Props, Clone, PartialEq)]
pub(super) struct EditableCellProps {
    pub col_class: String,
    pub cell_testid: String,
    pub field: EditField,
    pub display_value: String,
    /// Separate value used to seed the input when edit mode opens.
    /// Some cells render a richer display string than the underlying
    /// scalar override — Series shows "Pioneers #1" but only "Pioneers"
    /// is the series-name override. Pass `Some(bare_value)` so the
    /// input seeds (and the blur-comparison runs against) the editable
    /// scalar, not the rendered text. Defaults to `display_value`.
    #[props(default)]
    pub edit_value: Option<String>,
    pub is_admin: bool,
    pub editing: Signal<Option<EditField>>,
    pub placeholder: String,
    pub on_save: EventHandler<String>,
    pub error: Option<String>,
}

/// Inline-editable text cell used by `EbookRow`. Renders a span of text by
/// default; in admin mode, a click swaps to a text input that commits via the
/// `on_save` prop on Enter or blur and cancels on Escape. The input's `onclick`
/// stops propagation so the row-level navigate handler doesn't fire while the
/// user is editing.
#[component]
pub(super) fn EditableCell(props: EditableCellProps) -> Element {
    let EditableCellProps {
        col_class,
        cell_testid,
        field,
        display_value,
        edit_value,
        is_admin,
        editing,
        placeholder,
        on_save,
        error,
    } = props;
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

/// Props for the [`AuthorsCell`] component.
#[derive(Props, Clone, PartialEq)]
pub(super) struct AuthorsCellProps {
    /// Gates edit affordances — only admins can open the chip editor.
    pub is_admin: bool,
    /// Which field (if any) the row is currently editing.
    pub editing: Signal<Option<EditField>>,
    /// The in-progress author chip list shared with the row.
    pub authors_draft: Signal<Vec<String>>,
    /// Comma-joined author names shown in display mode.
    pub authors_text: String,
    /// Library-wide author pool surfaced in the dropdown.
    pub suggestions: ReadSignal<Vec<SuggestionItem>>,
    /// Fired with the new full author list on every add/remove.
    pub on_change: EventHandler<Vec<String>>,
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
pub(super) fn AuthorsCell(props: AuthorsCellProps) -> Element {
    let AuthorsCellProps {
        is_admin,
        editing,
        authors_draft,
        authors_text,
        suggestions,
        on_change,
    } = props;
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
                        on_change,
                        suggestions,
                        options: ChipEditorOptions {
                            placeholder: "+ add author\u{2026}".to_string(),
                            show_avatar: false,
                            aria_remove_prefix: "Remove".to_string(),
                            testid_prefix: "ebook-cell-author".to_string(),
                            autofocus: true,
                            dropdown_header: "ADD AUTHOR".to_string(),
                            ..ChipEditorOptions::default()
                        },
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
