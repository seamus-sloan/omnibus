//! Row-level components for the landing table.
//!
//! Hosts per-row state plumbing and the `EbookRow` → `EbookRowMarkup` →
//! `EbookRowCells` chain that `BookTable` in [`super`] renders per book.
//! Per-cell rendering lives in [`super::cells`].

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::super::sorting::{contributor_names, row_ident};
use super::cells::{
    build_save_authors, build_save_field, build_save_tags, CellEditCtx, ChipCell, ChipCellDisplay,
    EbookRowCoverCell, EbookRowFormatsCell, RowContext, RowScalarCell, RowScalarCellDisplay,
    RowSeriesCell, RowTitleCell,
};
use super::{BookTableContext, EditField};
use crate::Route;

/// One row in the power-user table for `book`.
#[component]
pub(super) fn EbookRow(book: EbookMetadata, ctx: BookTableContext) -> Element {
    let BookTableContext {
        server_url,
        is_admin,
        author_suggestions,
        tag_suggestions,
        selected,
    } = ctx;

    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let RowState {
        book_state,
        editing,
        authors_draft,
        tags_draft,
    } = use_row_state(book);
    let save_field = build_save_field(uuid.clone(), server_url.clone(), book_state);
    let save_authors = build_save_authors(uuid.clone(), server_url.clone(), book_state);
    let save_tags = build_save_tags(uuid.clone(), server_url.clone(), book_state);
    let cover_bust =
        crate::contexts::cover_bust_for(crate::contexts::use_cover_cache_bust().0, &uuid);
    let display = derive_row_display(&book_state.read(), &server_url, &uuid, cover_bust);

    let ctx = RowContext {
        is_admin,
        editing,
        authors_draft,
        tags_draft,
        author_suggestions,
        tag_suggestions,
        save_field: EventHandler::new(save_field),
        save_authors: EventHandler::new(save_authors),
        save_tags: EventHandler::new(save_tags),
        selected,
    };

    rsx! {
        EbookRowMarkup { display, uuid, ctx }
    }
}

/// Per-row reactive state — optimistic book copy, current editing cell, and
/// the authors/tags chip-editor drafts. The optimistic copy lets a successful
/// inline save update the row immediately without a full library refetch;
/// the `book` prop is resynced into it on upstream changes only when no
/// cell is mid-edit (so a save round-trip doesn't clobber an open input).
/// `authors_draft` / `tags_draft` mirror `book_state.creators` / `.subjects`
/// so each chip editor sees canonical values after a save without dropping
/// in-progress chip edits.
struct RowState {
    book_state: Signal<EbookMetadata>,
    editing: Signal<Option<EditField>>,
    authors_draft: Signal<Vec<String>>,
    tags_draft: Signal<Vec<String>>,
}

/// Seed and wire [`RowState`] for one row — see its doc for the semantics.
fn use_row_state(book: EbookMetadata) -> RowState {
    let mut book_state: Signal<EbookMetadata> = use_signal(|| book.clone());
    let editing: Signal<Option<EditField>> = use_signal(|| None);
    use_effect(use_reactive!(|book| {
        // `.peek()` avoids subscribing this effect to `editing`, so a save
        // that resolves after edit-close (e.g. the chip editor) can't have
        // its fresh `book_state` clobbered by a spurious resync.
        if editing.peek().is_none() {
            // `book` here is `use_reactive!`'s per-run dependency snapshot
            // (already an owned clone produced by the macro), not the
            // outer `book` parameter — safe to move straight into the
            // signal instead of cloning again.
            book_state.set(book);
        }
    }));

    let initial_authors: Vec<String> = book_state
        .read()
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let mut authors_draft: Signal<Vec<String>> = use_signal(|| initial_authors);
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

    let initial_tags = book_state.read().subjects.clone();
    let mut tags_draft: Signal<Vec<String>> = use_signal(|| initial_tags);
    use_effect(move || {
        let canonical = book_state.read().subjects.clone();
        if *tags_draft.peek() != canonical {
            tags_draft.set(canonical);
        }
    });

    RowState {
        book_state,
        editing,
        authors_draft,
        tags_draft,
    }
}

/// Pre-derived, ready-to-render display strings for a single row. Kept in
/// one struct so `EbookRowMarkup` reads cleanly and the row body stays
/// under the 80-line cap.
#[derive(Clone, PartialEq)]
struct RowDisplay {
    book: EbookMetadata,
    row_testid: String,
    thumb_src: String,
    thumb_srcset: String,
    has_cover: bool,
    title: String,
    series_line: String,
    series_text: String,
    authors_text: String,
    updated: String,
    added: String,
    tags_text: String,
    published: String,
    language: String,
}

/// Compute the per-cell display strings from the optimistic book state.
/// Pulled out of [`EbookRow`] so the closures and rsx live in their own
/// scopes. `cover_bust` is this book's [`crate::contexts::CoverCacheBust`]
/// counter (0 = unchanged this session) — `/api/thumbs/*` is cached
/// `private, max-age=86400`, so without it the table would keep showing a
/// pre-edit thumbnail for the otherwise-unchanged URL after navigating back
/// from a cover edit (issue #1087).
fn derive_row_display(
    book: &EbookMetadata,
    server_url: &str,
    uuid: &str,
    cover_bust: u32,
) -> RowDisplay {
    let row_testid = format!("ebook-row-{}", row_ident(book));
    // Per-variant URLs (not a shared base) so mobile's `?token=` attaches to
    // each `srcset` candidate; see `crate::thumb_url`.
    let bust = |url: String| crate::contexts::append_cache_bust(url, cover_bust);
    let sm = bust(crate::thumb_url(server_url, uuid, "sm"));
    let md = bust(crate::thumb_url(server_url, uuid, "md"));
    let lg = bust(crate::thumb_url(server_url, uuid, "lg"));
    let thumb_src = md.clone();
    let thumb_srcset = format!("{sm} 160w, {md} 320w, {lg} 640w");
    let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let series_line = match (book.series.as_deref(), book.series_index.as_deref()) {
        (Some(s), Some(i)) => format!("{s} #{i}"),
        (Some(s), None) => s.to_string(),
        _ => String::new(),
    };
    RowDisplay {
        row_testid,
        thumb_src,
        thumb_srcset,
        has_cover: book.cover_url.is_some(),
        title,
        series_line,
        series_text: book.series.clone().unwrap_or_default(),
        authors_text: contributor_names(&book.creators),
        updated: book.modified.as_deref().unwrap_or("").to_string(),
        added: book.added_at.as_deref().unwrap_or("").to_string(),
        tags_text: book.subjects.join(", "),
        published: book.published.clone().unwrap_or_default(),
        language: book.language.clone().unwrap_or_default(),
        book: book.clone(),
    }
}

/// Inner row markup — the `<tr>` + every `<td>`. Split out of [`EbookRow`]
/// so the parent stays a thin state-setup shell. All inputs are already
/// derived; this component does no signal seeding of its own.
#[component]
fn EbookRowMarkup(display: RowDisplay, uuid: String, ctx: RowContext) -> Element {
    let mut editing = ctx.editing;
    let nav = use_navigator();
    let uuid_click = uuid.clone();
    let uuid_key = uuid;
    let row_testid = display.row_testid.clone();
    let aria_title = display.title.clone();

    rsx! {
        tr {
            class: "ebook-row",
            "data-testid": "{row_testid}",
            id: "{row_testid}",
            role: "button",
            tabindex: "0",
            aria_label: "Open details for {aria_title}",
            onclick: move |_| {
                // Row-level click navigates only when no cell-level edit
                // is in progress. Each editable cell stops propagation on
                // click already, but a click on a non-editable area
                // (cover, formats, dates) while a cell is open would
                // otherwise blur+save the input AND fire the navigation.
                // Bailing on the editing-is-some path keeps the user on
                // the page while the blur-save lands.
                if editing().is_some() {
                    return;
                }
                nav.push(Route::BookDetail { uuid: uuid_click.clone() });
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                if editing().is_some() {
                    // Fallback close: the open cell's own input handles
                    // Escape (and stops propagation) when it holds focus,
                    // but focus can end up elsewhere in the row — a chip's
                    // remove button, or a fresh editor whose autofocus
                    // didn't land — leaving `editing` stuck with no input
                    // listening for Escape. Catch it here so the amber
                    // highlight can always be dismissed.
                    if evt.key() == Key::Escape {
                        editing.set(None);
                    }
                    return;
                }
                let key = evt.key();
                if key == Key::Enter || key == Key::Character(" ".to_string()) {
                    evt.prevent_default();
                    nav.push(Route::BookDetail { uuid: uuid_key.clone() });
                }
            },
            EbookRowCells { display, ctx }
        }
    }
}

/// Flat list of `<td>` cells inside an [`EbookRow`]. Split out of
/// [`EbookRowMarkup`] so the row-level event handlers and the per-cell
/// wiring live in separate functions.
#[component]
fn EbookRowCells(display: RowDisplay, ctx: RowContext) -> Element {
    let RowContext {
        is_admin,
        editing,
        authors_draft,
        tags_draft,
        author_suggestions,
        tag_suggestions,
        save_field,
        save_authors,
        save_tags,
        mut selected,
    } = ctx;
    // Cloned per scalar cell so each wrapper owns its own copy of the shared
    // admin/editing/save context.
    let cell_ctx = CellEditCtx {
        is_admin,
        editing,
        save_field,
    };
    let RowDisplay {
        book,
        row_testid: _,
        thumb_src,
        thumb_srcset,
        has_cover,
        title,
        series_line,
        series_text,
        authors_text,
        updated,
        added,
        tags_text,
        published,
        language,
    } = display;
    let cover_alt = title.clone();
    let select_uuid = book.unique_identifier.clone().unwrap_or_default();
    let is_selected = selected.read().contains(&select_uuid);
    let select_aria = format!("Select {title}");

    rsx! {
        if is_admin {
            // `stop_propagation` on the whole cell (not just the checkbox) so a
            // near-miss click can't fire the row's navigate handler.
            td {
                class: "ebook-col-select",
                onclick: move |evt| evt.stop_propagation(),
                input {
                    r#type: "checkbox",
                    "data-testid": "ebook-select",
                    aria_label: "{select_aria}",
                    checked: is_selected,
                    onchange: move |_| {
                        let mut set = selected.write();
                        if !set.remove(&select_uuid) {
                            set.insert(select_uuid.clone());
                        }
                    },
                }
            }
        }
        EbookRowCoverCell { thumb_src, thumb_srcset, has_cover, alt_title: cover_alt }
        RowTitleCell { title, error: book.error, ctx: cell_ctx.clone() }
        ChipCell {
            display: ChipCellDisplay::authors(authors_text),
            is_admin,
            editing,
            draft: authors_draft,
            suggestions: author_suggestions,
            on_change: move |names: Vec<String>| save_authors.call(names),
        }
        RowSeriesCell { series_line, series_text, ctx: cell_ctx.clone() }
        ChipCell {
            display: ChipCellDisplay::tags(tags_text),
            is_admin,
            editing,
            draft: tags_draft,
            suggestions: tag_suggestions,
            on_change: move |tags: Vec<String>| save_tags.call(tags),
        }
        RowScalarCell {
            display: RowScalarCellDisplay {
                col_class: "ebook-col-published".to_string(),
                cell_testid: "ebook-cell-published".to_string(),
                value: published,
                placeholder: "YYYY-MM-DD".to_string(),
            },
            field: EditField::Published,
            ctx: cell_ctx.clone(),
        }
        EbookRowFormatsCell { formats: book.formats, has_physical: book.has_physical }
        td { class: "ebook-col-updated", "data-testid": "ebook-cell-updated", "{updated}" }
        td { class: "ebook-col-added", "data-testid": "ebook-cell-added", "{added}" }
        RowScalarCell {
            display: RowScalarCellDisplay {
                col_class: "ebook-col-language".to_string(),
                cell_testid: "ebook-cell-language".to_string(),
                value: language,
                placeholder: "en".to_string(),
            },
            field: EditField::Language,
            ctx: cell_ctx,
        }
    }
}
