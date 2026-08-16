//! Signal setup, dirty tracking, and save/revert handlers for
//! `MetadataEditForm`, hoisted out of the component body so the render
//! function itself stays a straight composition of the sub-panels.

use dioxus::prelude::*;
use dioxus_router::navigator;
use omnibus_shared::EbookMetadata;

use super::form_grid::{FormFields, FormSuggestions};
use super::save_bar::{DirtyState, SaveStatus};
use super::{build_overrides, EditedFields};
use crate::components::chip_editor::{collect_suggestions, SuggestionItem};
use crate::{data, Route};

/// Everything `MetadataEditForm`'s rsx needs to render: the per-field
/// signals bundled into the sub-panel prop structs, dirty tracking, the
/// save/revert handlers, and the resolved header display strings.
pub(super) struct EditFormState {
    pub(super) orig: Signal<EbookMetadata>,
    pub(super) fields: FormFields,
    pub(super) suggestions: FormSuggestions,
    pub(super) dirty: DirtyState,
    pub(super) status: SaveStatus,
    pub(super) on_save: EventHandler<()>,
    pub(super) on_revert: EventHandler<()>,
    pub(super) display_title: String,
    pub(super) primary_author: String,
    pub(super) primary_author_id: Option<i64>,
    pub(super) accent_style: String,
}

/// Sets up all per-field signals, the suggestion-pool fetches, dirty
/// tracking, and the save/revert handlers backing `MetadataEditForm`.
pub(super) fn use_metadata_edit_form_state(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> EditFormState {
    // Original values — frozen snapshot for dirty comparison.
    let orig = use_signal(|| book.clone());
    let fields = use_field_signals(book);
    let suggestions = use_suggestion_pools(server_url);

    // Save / error state.
    let saving = use_signal(|| false);
    let save_error: Signal<Option<String>> = use_signal(|| None);

    let dirty_fields = use_dirty_fields(orig, fields);
    let dirty_count = use_memo(move || dirty_fields().len());

    let (display_title, primary_author, primary_author_id, accent_style) = header_strings(book);

    let on_revert = build_on_revert(server_url, uuid, saving, save_error);
    let on_save = build_on_save(server_url, uuid, saving, save_error, orig, fields);

    EditFormState {
        orig,
        fields,
        suggestions,
        dirty: DirtyState {
            fields: dirty_fields,
            count: dirty_count,
        },
        status: SaveStatus {
            saving,
            error: save_error,
        },
        on_save,
        on_revert,
        display_title,
        primary_author,
        primary_author_id,
        accent_style,
    }
}

/// Per-field editable signals seeded from the loaded book, plus the two
/// read-only display signals (`sort_by`, `filename`) the form grid renders
/// alongside them.
fn use_field_signals(book: &EbookMetadata) -> FormFields {
    let title = use_signal(|| book.title.clone().unwrap_or_default());
    let description = use_signal(|| book.description.clone().unwrap_or_default());
    let publisher = use_signal(|| book.publisher.clone().unwrap_or_default());
    let published = use_signal(|| book.published.clone().unwrap_or_default());
    let language = use_signal(|| book.language.clone().unwrap_or_default());
    let series = use_signal(|| book.series.clone().unwrap_or_default());
    let series_index = use_signal(|| book.series_index.clone().unwrap_or_default());
    let isbn13 = use_signal(|| book.isbn13.clone().unwrap_or_default());
    let isbn10 = use_signal(|| book.isbn10.clone().unwrap_or_default());
    let print_pages = use_signal(|| book.print_pages.map(|p| p.to_string()).unwrap_or_default());

    // Authors as a signal of Vec<String> (names only for v1).
    let authors = use_signal(|| {
        book.creators
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
    });

    // Tags (subjects) as a signal of Vec<String>.
    let tags = use_signal(|| book.subjects.clone());

    // Genres — user-assigned only, so this seeds from whatever the
    // override layer supplied (empty for a book nobody has genred).
    let genres = use_signal(|| book.genres.clone());

    // Read-only field signals — hoisted here so `use_signal` isn't called
    // inside the `rsx!` body on every render.
    let sort_by = use_signal(|| {
        book.creators
            .first()
            .map(|c| c.file_as.clone().unwrap_or_else(|| c.name.clone()))
            .unwrap_or_default()
    });
    let filename = use_signal(|| book.filename.clone());

    FormFields {
        title,
        description,
        publisher,
        published,
        language,
        series,
        series_index,
        isbn13,
        isbn10,
        print_pages,
        authors,
        tags,
        genres,
        sort_by,
        filename,
    }
}

/// Header display strings derived once from the loaded book: the resolved
/// title, primary author name + id, and the CSS custom-property style
/// string for the page's accent color.
fn header_strings(book: &EbookMetadata) -> (String, String, Option<i64>, String) {
    let title = book.display_title();
    let (primary_author, primary_author_id) = book
        .creators
        .first()
        .map(|c| (c.name.clone(), c.id))
        .unwrap_or_default();
    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    (title, primary_author, primary_author_id, accent_style)
}

/// Fetches the author/tag/genre/series suggestion pools once on mount for the
/// `ChipEditor`/`SuggestField` dropdowns; signals stay empty until the
/// fetches resolve.
fn use_suggestion_pools(server_url: &str) -> FormSuggestions {
    let mut author_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut tag_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut genre_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut series_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);

    let url = server_url.to_string();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            if let Ok(authors) = data::list_authors(&url).await {
                let items: Vec<SuggestionItem> = authors
                    .into_iter()
                    .map(|a| SuggestionItem::new(a.name, a.book_count))
                    .collect();
                author_suggestions.set(collect_suggestions(items));
            }
            if let Ok(tags) = data::get_tag_cloud(&url).await {
                let items: Vec<SuggestionItem> = tags
                    .into_iter()
                    .map(|t| SuggestionItem::new(t.name, t.count))
                    .collect();
                tag_suggestions.set(collect_suggestions(items));
            }
            if let Ok(genres) = data::get_genre_cloud(&url).await {
                let items: Vec<SuggestionItem> = genres
                    .into_iter()
                    .map(|g| SuggestionItem::new(g.name, g.count))
                    .collect();
                genre_suggestions.set(collect_suggestions(items));
            }
            if let Ok(series) = data::list_series(&url).await {
                let items: Vec<SuggestionItem> = series
                    .into_iter()
                    .map(|s| SuggestionItem::new(s.name, s.book_count))
                    .collect();
                series_suggestions.set(collect_suggestions(items));
            }
        });
    });

    FormSuggestions {
        authors: author_suggestions,
        tags: tag_suggestions,
        genres: genre_suggestions,
        series: series_suggestions,
    }
}

/// Dirty-field tracking: which display labels differ from `orig`, driving
/// the save bar's field-count badge.
fn use_dirty_fields(orig: Signal<EbookMetadata>, fields: FormFields) -> Memo<Vec<&'static str>> {
    let FormFields {
        title,
        description,
        publisher,
        published,
        language,
        series,
        series_index,
        isbn13,
        isbn10,
        print_pages,
        authors,
        tags,
        genres,
        sort_by: _,
        filename: _,
    } = fields;
    use_memo(move || {
        let o = orig();
        let mut dirty: Vec<&str> = Vec::new();
        if title() != o.title.clone().unwrap_or_default() {
            dirty.push("Title");
        }
        if description() != o.description.clone().unwrap_or_default() {
            dirty.push("Description");
        }
        if publisher() != o.publisher.clone().unwrap_or_default() {
            dirty.push("Publisher");
        }
        if published() != o.published.clone().unwrap_or_default() {
            dirty.push("Published");
        }
        if language() != o.language.clone().unwrap_or_default() {
            dirty.push("Language");
        }
        if series() != o.series.clone().unwrap_or_default() {
            dirty.push("Series");
        }
        if series_index() != o.series_index.clone().unwrap_or_default() {
            dirty.push("Book #");
        }
        if isbn13() != o.isbn13.clone().unwrap_or_default() {
            dirty.push("ISBN-13");
        }
        if isbn10() != o.isbn10.clone().unwrap_or_default() {
            dirty.push("ISBN-10");
        }
        if print_pages().trim() != o.print_pages.map(|p| p.to_string()).unwrap_or_default() {
            dirty.push("Print Pages");
        }
        let orig_authors: Vec<String> = o.creators.iter().map(|c| c.name.clone()).collect();
        if authors() != orig_authors {
            dirty.push("Authors");
        }
        if tags() != o.subjects {
            dirty.push("Tags");
        }
        if genres() != o.genres {
            dirty.push("Genres");
        }
        dirty
    })
}

/// Revert handler — deletes overrides and navigates back to the book
/// detail page on success.
fn build_on_revert(
    server_url: &str,
    uuid: &str,
    mut saving: Signal<bool>,
    mut save_error: Signal<Option<String>>,
) -> EventHandler<()> {
    let url = server_url.to_string();
    let uuid = uuid.to_string();
    EventHandler::new(move |()| {
        let url = url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            saving.set(true);
            save_error.set(None);
            match data::delete_overrides(&url, &uuid).await {
                Ok(_) => {
                    let nav = navigator();
                    nav.push(Route::BookDetail { uuid: uuid.clone() });
                }
                Err(e) => save_error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    })
}

/// Save handler — builds the diff and POSTs to the overrides endpoint,
/// then navigates back to the book detail page on success.
fn build_on_save(
    server_url: &str,
    uuid: &str,
    mut saving: Signal<bool>,
    mut save_error: Signal<Option<String>>,
    orig: Signal<EbookMetadata>,
    fields: FormFields,
) -> EventHandler<()> {
    let url = server_url.to_string();
    let uuid = uuid.to_string();
    let FormFields {
        title,
        description,
        publisher,
        published,
        language,
        series,
        series_index,
        isbn13,
        isbn10,
        print_pages,
        authors,
        tags,
        genres,
        sort_by: _,
        filename: _,
    } = fields;
    EventHandler::new(move |()| {
        let url = url.clone();
        let uuid = uuid.clone();

        // Parsed client-side so a non-numeric entry gets an immediate,
        // specific message instead of a generic save failure (mirrors
        // `ScanIntervalField`'s handler in settings/library.rs); an in-range
        // numeric value still round-trips through `MetadataOverrides::validate`
        // server-side (e.g. the `PRINT_PAGES_MAX` ceiling).
        let print_pages_input = print_pages().trim().to_string();
        let print_pages_val: Option<i64> = if print_pages_input.is_empty() {
            None
        } else {
            match print_pages_input.parse::<i64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    save_error.set(Some("Print page count must be a whole number.".to_string()));
                    return;
                }
            }
        };

        spawn(async move {
            saving.set(true);
            save_error.set(None);

            let o = orig();
            let overrides = build_overrides(
                &o,
                EditedFields {
                    title: &title(),
                    description: &description(),
                    publisher: &publisher(),
                    published: &published(),
                    language: &language(),
                    series: &series(),
                    series_index: &series_index(),
                    isbn13: &isbn13(),
                    isbn10: &isbn10(),
                    print_pages: print_pages_val,
                    authors: &authors(),
                    tags: &tags(),
                    genres: &genres(),
                },
            );

            match data::save_overrides(&url, &uuid, &overrides).await {
                Ok(_) => {
                    let nav = navigator();
                    nav.push(Route::BookDetail { uuid: uuid.clone() });
                }
                Err(e) => save_error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    })
}
