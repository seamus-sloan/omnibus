//! Row-level components for the landing table.
//!
//! Hosts per-row state plumbing and the `EbookRow` → `EbookRowMarkup` →
//! `EbookRowCells` chain that `BookTable` in [`super`] renders per book.
//! Per-cell rendering lives in [`super::cells`].

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::super::sorting::{contributor_names, row_slug};
use super::cells::{
    build_save_authors, build_save_field, AuthorsCell, CellEditCtx, EbookRowCoverCell,
    EbookRowFormatsCell, RowContext, RowScalarCell, RowSeriesCell, RowTitleCell,
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
        tag_suggestions: _tag_suggestions,
    } = ctx;

    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let (book_state, editing, authors_draft) = use_row_state(book.clone());
    let save_field = build_save_field(uuid.clone(), server_url.clone(), book_state);
    let save_authors = build_save_authors(uuid.clone(), server_url.clone(), book_state);
    let display = derive_row_display(&book_state.read(), &server_url, &uuid);

    let ctx = RowContext {
        is_admin,
        editing,
        authors_draft,
        author_suggestions,
        save_field: EventHandler::new(save_field),
        save_authors: EventHandler::new(save_authors),
    };

    rsx! {
        EbookRowMarkup { display, uuid, ctx }
    }
}

/// Per-row reactive state — optimistic book copy, current editing cell, and
/// the authors chip-editor draft. The optimistic copy lets a successful
/// inline save update the row immediately without a full library refetch;
/// the `book` prop is resynced into it on upstream changes only when no
/// cell is mid-edit (so a save round-trip doesn't clobber an open input).
/// `authors_draft` mirrors `book_state.creators` so the chip editor sees
/// canonical names after a save without dropping in-progress chip edits.
fn use_row_state(
    book: EbookMetadata,
) -> (
    Signal<EbookMetadata>,
    Signal<Option<EditField>>,
    Signal<Vec<String>>,
) {
    let mut book_state: Signal<EbookMetadata> = use_signal(|| book.clone());
    let editing: Signal<Option<EditField>> = use_signal(|| None);
    use_effect(use_reactive!(|book| {
        if editing().is_none() {
            book_state.set(book.clone());
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

    (book_state, editing, authors_draft)
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
    publisher: String,
    published: String,
    language: String,
}

/// Compute the per-cell display strings from the optimistic book state.
/// Pulled out of [`EbookRow`] so the closures and rsx live in their own
/// scopes.
fn derive_row_display(book: &EbookMetadata, server_url: &str, uuid: &str) -> RowDisplay {
    let row_testid = format!("ebook-row-{}", row_slug(&book.filename));
    // Per-variant URLs (not a shared base) so mobile's `?token=` attaches to
    // each `srcset` candidate; see `crate::thumb_url`.
    let sm = crate::thumb_url(server_url, uuid, "sm");
    let md = crate::thumb_url(server_url, uuid, "md");
    let lg = crate::thumb_url(server_url, uuid, "lg");
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
        publisher: book.publisher.clone().unwrap_or_default(),
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
    let editing = ctx.editing;
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
        author_suggestions,
        save_field,
        save_authors,
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
        publisher,
        published,
        language,
    } = display;
    let cover_alt = title.clone();

    rsx! {
        EbookRowCoverCell { thumb_src, thumb_srcset, has_cover, alt_title: cover_alt }
        RowTitleCell { title, error: book.error, ctx: cell_ctx.clone() }
        AuthorsCell {
            is_admin,
            editing,
            authors_draft,
            authors_text,
            suggestions: author_suggestions,
            on_change: move |names: Vec<String>| save_authors.call(names),
        }
        RowSeriesCell { series_line, series_text, ctx: cell_ctx.clone() }
        RowScalarCell {
            col_class: "ebook-col-publisher".to_string(),
            cell_testid: "ebook-cell-publisher".to_string(),
            field: EditField::Publisher,
            value: publisher,
            placeholder: "Publisher".to_string(),
            ctx: cell_ctx.clone(),
        }
        RowScalarCell {
            col_class: "ebook-col-published".to_string(),
            cell_testid: "ebook-cell-published".to_string(),
            field: EditField::Published,
            value: published,
            placeholder: "YYYY-MM-DD".to_string(),
            ctx: cell_ctx.clone(),
        }
        EbookRowFormatsCell { formats: book.formats }
        td { class: "ebook-col-updated", "data-testid": "ebook-cell-updated", "{updated}" }
        td { class: "ebook-col-added", "data-testid": "ebook-cell-added", "{added}" }
        RowScalarCell {
            col_class: "ebook-col-language".to_string(),
            cell_testid: "ebook-cell-language".to_string(),
            field: EditField::Language,
            value: language,
            placeholder: "en".to_string(),
            ctx: cell_ctx,
        }
    }
}
