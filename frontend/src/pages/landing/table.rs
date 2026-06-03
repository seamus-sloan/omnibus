use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides, SortDir, SortKey, ViewPrefs};

use super::filtering::format_badge_label;
use super::sorting::{contributor_names, row_slug};
use crate::components::chip_editor::{ChipEditor, SuggestionItem};
use crate::{data, Route};

/// Inline-editable field on a power-user-table row. Used by `EbookRow` to
/// track which of its cells (if any) is in edit mode at any given time —
/// single-cell-at-a-time keeps the keyboard/blur lifecycle simple and
/// avoids fighting the row's click-to-navigate handler.
///
/// `Series` edits the series *name* only — the series index ("#N") stays
/// in the full metadata edit page on `/books/:uuid/edit` to avoid adding a
/// separate Series Index column to the power-user table. Title + Series +
/// Authors covers the stated cleanup pain (stripping series-prefix cruft
/// from titles, renaming author/series variants).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditField {
    Title,
    Series,
    Publisher,
    Published,
    Language,
    Authors,
}

#[component]
pub(super) fn BookTable(
    books: Vec<EbookMetadata>,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
    server_url: String,
    is_admin: bool,
    // Forwarded by reference (ReadSignal) all the way down so the
    // suggestion pool isn't cloned at any intermediate layer — even on
    // libraries with thousands of authors.
    author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
) -> Element {
    rsx! {
        div {
            id: "ebook-table",
            "data-testid": "ebook-table",
            class: "ebook-table-wrap",
            table { class: "ebook-table",
                thead {
                    tr {
                        th { class: "ebook-col-cover", "Cover" }
                        SortableHeader {
                            class: "ebook-col-title".to_string(),
                            label: "Title".to_string(),
                            sort_key: SortKey::Title,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-author".to_string(),
                            label: "Author".to_string(),
                            sort_key: SortKey::Author,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-series".to_string(),
                            label: "Series".to_string(),
                            sort_key: SortKey::Series,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-publisher", "Publisher" }
                        th { class: "ebook-col-published", "Published" }
                        th { class: "ebook-col-formats", "Formats" }
                        SortableHeader {
                            class: "ebook-col-updated".to_string(),
                            label: "Last Updated".to_string(),
                            sort_key: SortKey::LastUpdated,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-added".to_string(),
                            label: "Added".to_string(),
                            sort_key: SortKey::NewestAdded,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-language", "Language" }
                    }
                }
                tbody {
                    for book in books.into_iter() {
                        EbookRow {
                            key: "{book.filename}",
                            book: book,
                            server_url: server_url.clone(),
                            is_admin,
                            author_suggestions,
                            tag_suggestions,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SortableHeader(
    class: String,
    label: String,
    sort_key: SortKey,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
) -> Element {
    let active = prefs.sort_key == sort_key;
    let aria_sort = match (active, prefs.sort_dir) {
        (true, SortDir::Asc) => "ascending",
        (true, SortDir::Desc) => "descending",
        _ => "none",
    };
    let arrow = if !active {
        ""
    } else if prefs.sort_dir == SortDir::Asc {
        " ↑"
    } else {
        " ↓"
    };
    rsx! {
        th { class: "{class} sort-th", aria_sort: "{aria_sort}",
            button {
                class: "sort-th-btn",
                onclick: move |_| on_sort.call(sort_key),
                "{label}{arrow}"
            }
        }
    }
}

#[component]
fn EbookRow(
    book: EbookMetadata,
    server_url: String,
    is_admin: bool,
    author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
) -> Element {
    // Stable per-book uuid drives both the detail-route URL and the
    // thumbnail URL — see `Route::BookDetail` for why it's keyed on the
    // uuid instead of `books.id`.
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let row_testid = format!("ebook-row-{}", row_slug(&book.filename));
    let has_cover = book.cover_url.is_some();
    let thumb_base = format!("{server_url}/api/thumbs/{uuid}");
    let _ = tag_suggestions; // reserved for a future inline-tag editor

    // Optimistic per-row state: we render from `book_state` rather than
    // the raw `book` prop so a successful inline save updates the row
    // immediately, without a full library refetch. The server-side
    // merge in `rpc_save_overrides` returns the canonical merged
    // EbookMetadata which we install verbatim.
    //
    // Resync from the prop when the upstream `book` changes (library
    // refetch / sort change reusing the same EbookRow instance via the
    // filename key) — but only when no cell is mid-edit, so a save
    // round-trip doesn't clobber an open input. `use_reactive!` keys
    // the effect on `book` itself.
    let mut book_state: Signal<EbookMetadata> = use_signal(|| book.clone());
    let editing: Signal<Option<EditField>> = use_signal(|| None);
    use_effect(use_reactive!(|book| {
        if editing().is_none() {
            book_state.set(book.clone());
        }
    }));

    let nav = use_navigator();
    let uuid_click = uuid.clone();
    let uuid_key = uuid.clone();
    let uuid_for_save = uuid.clone();
    let url_for_save = server_url.clone();

    // Common save callback used by every inline cell. Builds a sparse
    // `MetadataOverrides`, POSTs to the existing F5.1 endpoint, then
    // installs the server-returned merged metadata into `book_state`.
    // Empty strings clear the override for the field (None — the
    // scanned value re-surfaces); non-empty strings persist as
    // `Some(value)`.
    let save_field = move |field: EditField, value: String| {
        let uuid = uuid_for_save.clone();
        let url = url_for_save.clone();
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
                EditField::Authors => {
                    // Authors is handled below — this branch is reserved
                    // for the chip-editor on_change path which sets
                    // `creators` instead.
                    return;
                }
            }
            if overrides.validate().is_err() {
                // Validation rejects payloads that exceed the F5.1
                // length caps. The display reverts on the next
                // refetch; a toast / inline error message ships in a
                // follow-up PR.
                return;
            }
            if let Ok(Some(merged)) = data::save_overrides(&url, &uuid, &overrides).await {
                book_state.set(merged);
            }
        });
    };
    // Authors chip editor lifts the canonical creator list out of the
    // optimistic row state into a Signal so ChipEditor can drive it
    // directly. Edits go through `save_authors` (separate callback so
    // it can build the `creators` override; the text-cell path is
    // scalar-only).
    let initial_authors: Vec<String> = book_state
        .read()
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let mut authors_draft: Signal<Vec<String>> = use_signal(|| initial_authors);
    // Keep `authors_draft` in sync with `book_state.creators` so a
    // save round-trip (which may canonicalize / reorder names on the
    // server) doesn't leave the chip editor showing stale chips, and
    // re-opening the cell after an external update shows the current
    // truth. The effect is a no-op when the values already match, so
    // the user's in-progress chip edits aren't clobbered between
    // `on_change` and the optimistic `book_state` install.
    use_effect(move || {
        let canonical: Vec<String> = book_state
            .read()
            .creators
            .iter()
            .map(|c| c.name.clone())
            .collect();
        if *authors_draft.peek() != canonical {
            authors_draft.set(canonical);
        }
    });
    let uuid_for_authors = uuid.clone();
    let url_for_authors = server_url.clone();
    let save_authors = move |new_names: Vec<String>| {
        let uuid = uuid_for_authors.clone();
        let url = url_for_authors.clone();
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
    };

    // Pull values from the optimistic state so a save's returned
    // merged metadata is what we render.
    let display_book = book_state.read().clone();
    let display_title = display_book
        .title
        .as_deref()
        .unwrap_or(&display_book.filename)
        .to_string();
    let series_line = match (
        display_book.series.as_deref(),
        display_book.series_index.as_deref(),
    ) {
        (Some(s), Some(i)) => format!("{s} #{i}"),
        (Some(s), None) => s.to_string(),
        _ => String::new(),
    };
    let authors_text = contributor_names(&display_book.creators);
    let updated = display_book.modified.as_deref().unwrap_or("").to_string();
    let added = display_book.added_at.as_deref().unwrap_or("").to_string();
    let publisher = display_book.publisher.clone().unwrap_or_default();
    let published = display_book.published.clone().unwrap_or_default();
    let language = display_book.language.clone().unwrap_or_default();
    let series_text = display_book.series.clone().unwrap_or_default();

    rsx! {
        tr {
                class: "ebook-row",
                "data-testid": "{row_testid}",
                id: "{row_testid}",
                role: "button",
                tabindex: "0",
                aria_label: "Open details for {display_title}",
                onclick: move |_| {
                    // Row-level click navigates only when no cell-level
                    // edit is in progress. Each editable cell stops
                    // propagation on click already, but a click on a
                    // non-editable area (cover, formats, dates) while
                    // a cell is open would otherwise blur+save the
                    // input AND fire the navigation. Bailing on the
                    // editing-is-some path keeps the user on the page
                    // while the blur-save lands.
                    if editing().is_some() {
                        return;
                    }
                    nav.push(Route::BookDetail { uuid: uuid_click.clone() });
                },
                onkeydown: move |evt: Event<KeyboardData>| {
                    if editing().is_some() {
                        return;
                    }
                    let key = evt.key();
                    if key == Key::Enter || key == Key::Character(" ".to_string()) {
                        evt.prevent_default();
                        nav.push(Route::BookDetail { uuid: uuid_key.clone() });
                    }
                },
                td { class: "ebook-col-cover", "data-testid": "ebook-cell-cover",
                    if has_cover {
                        img {
                            class: "ebook-thumb",
                            src: "{thumb_base}/md",
                            srcset: "{thumb_base}/sm 160w, {thumb_base}/md 320w, {thumb_base}/lg 640w",
                            sizes: "(max-width: 640px) 160px, (max-width: 1280px) 320px, 640px",
                            alt: "Cover of {display_title}",
                            loading: "lazy",
                            width: "320",
                            height: "480",
                        }
                    } else {
                        div { class: "ebook-thumb ebook-thumb-fallback", "—" }
                    }
                }
                EditableCell {
                    col_class: "ebook-col-title".to_string(),
                    cell_testid: "ebook-cell-title".to_string(),
                    field: EditField::Title,
                    display_value: display_title.clone(),
                    is_admin,
                    editing,
                    placeholder: "Title".to_string(),
                    on_save: { let save = save_field.clone(); move |v: String| save(EditField::Title, v) },
                    error: display_book.error.clone(),
                }
                AuthorsCell {
                    is_admin,
                    editing,
                    authors_draft,
                    authors_text: authors_text.clone(),
                    suggestions: author_suggestions,
                    on_change: {
                        let save = save_authors;
                        move |names: Vec<String>| save(names)
                    },
                }
                EditableCell {
                    col_class: "ebook-col-series".to_string(),
                    cell_testid: "ebook-cell-series".to_string(),
                    field: EditField::Series,
                    // Display: "Pioneers #1" (name + index, F1.3 standard).
                    // Edit:    "Pioneers"    (the series-name override is
                    //                         scalar; index lives in
                    //                         `series_index` which the
                    //                         landing-table doesn't
                    //                         currently expose for edit).
                    display_value: series_line.clone(),
                    edit_value: Some(series_text.clone()),
                    is_admin,
                    editing,
                    placeholder: "Series".to_string(),
                    on_save: { let save = save_field.clone(); move |v: String| save(EditField::Series, v) },
                    error: None,
                }
                EditableCell {
                    col_class: "ebook-col-publisher".to_string(),
                    cell_testid: "ebook-cell-publisher".to_string(),
                    field: EditField::Publisher,
                    display_value: publisher.clone(),
                    is_admin,
                    editing,
                    placeholder: "Publisher".to_string(),
                    on_save: { let save = save_field.clone(); move |v: String| save(EditField::Publisher, v) },
                    error: None,
                }
                EditableCell {
                    col_class: "ebook-col-published".to_string(),
                    cell_testid: "ebook-cell-published".to_string(),
                    field: EditField::Published,
                    display_value: published.clone(),
                    is_admin,
                    editing,
                    placeholder: "YYYY-MM-DD".to_string(),
                    on_save: { let save = save_field.clone(); move |v: String| save(EditField::Published, v) },
                    error: None,
                }
                td { class: "ebook-col-formats", "data-testid": "ebook-cell-formats",
                    if display_book.formats.is_empty() {
                        span { class: "ebook-cell-formats-empty", "—" }
                    } else {
                        for fmt in display_book.formats.iter() {
                            span { class: "format-badge", "{format_badge_label(fmt)}" }
                        }
                    }
                }
                td { class: "ebook-col-updated", "data-testid": "ebook-cell-updated", "{updated}" }
                td { class: "ebook-col-added", "data-testid": "ebook-cell-added", "{added}" }
                EditableCell {
                    col_class: "ebook-col-language".to_string(),
                    cell_testid: "ebook-cell-language".to_string(),
                    field: EditField::Language,
                    display_value: language.clone(),
                    is_admin,
                    editing,
                    placeholder: "en".to_string(),
                    on_save: { let save = save_field.clone(); move |v: String| save(EditField::Language, v) },
                    error: None,
                }
            }
    }
}

/// Inline-editable text cell used by [`EbookRow`]. Renders a span of
/// text by default; in admin mode, a click swaps to a text input that
/// commits via [`EditField::on_save`] on Enter or blur and cancels on
/// Escape. The input's `onclick` stops propagation so the row-level
/// navigate handler doesn't fire while the user is editing.
///
/// `suppress_inline_input` is the Authors-cell escape hatch: the click
/// still toggles `editing` (so the expansion row appears) but the
/// in-cell input never renders.
#[component]
fn EditableCell(
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
fn AuthorsCell(
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
