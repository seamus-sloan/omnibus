//! Single-book metadata edit form at `/books/:uuid/edit`. Two-column
//! layout (form grid + sticky cover/identifiers sidebar) with a sticky
//! save bar showing the dirty field count. Edits persist via
//! [`data::save_overrides`]; the `dirty_count` memo compares each per-field
//! signal to the original merged [`EbookMetadata`] loaded on mount.

use dioxus::prelude::*;
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use crate::components::{PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

mod cover_editor;
mod fields;
mod form_grid;
mod header;
mod metadata_search;
mod save_bar;
mod sidebar;
mod state;

use form_grid::FormGrid;
use header::{Breadcrumb, PageHeader};
use save_bar::SaveBar;
use sidebar::Sidebar;

/// Top-level metadata edit page component, mounted at `/books/:uuid/edit`.
#[component]
pub fn MetadataEditPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // See `BookDetailPage` for why `uuid` needs `use_reactive!`.
    let url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let url = url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            loading.set(true);
            match data::get_ebook(&url, &uuid).await {
                Ok(b) => {
                    book.set(b);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    }));

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! {
            PageError {
                message: msg,
                back_to: Route::BookDetail { uuid: uuid.clone() },
                back_label: "Back to book",
            }
        };
    }
    let Some(b) = book() else {
        return rsx! { PageNotFound { subject: "Book", back_to: Route::Landing {} } };
    };

    rsx! {
        MetadataEditForm { book: b, uuid }
    }
}

/// Edit form — the loaded-book case. Delegates signal setup, dirty
/// tracking, and the save/revert handlers to
/// [`state::use_metadata_edit_form_state`], then renders composition over
/// the page-local sub-components in `fields`/`form_grid`/`header`/
/// `sidebar`/`save_bar`.
#[component]
fn MetadataEditForm(book: EbookMetadata, uuid: String) -> Element {
    let server_url = use_server_url();
    let form = state::use_metadata_edit_form_state(&book, &uuid, &server_url);
    // The book as the server last reported it. Distinct from `form.orig`,
    // which is the frozen dirty-tracking baseline: this one moves when a
    // write lands outside the save bar — today only a cover, which is the one
    // edit on this page that doesn't wait for Save.
    let mut live_book = use_signal(|| book.clone());

    rsx! {
        div { class: "me-root", style: "{form.accent_style}",
            Breadcrumb {
                uuid: uuid.clone(),
                display_title: form.display_title.clone(),
                primary_author: form.primary_author.clone(),
                primary_author_id: form.primary_author_id,
            }
            PageHeader {
                display_title: form.display_title,
                primary_author: form.primary_author,
            }

            div { class: "me-layout",
                FormGrid {
                    orig: form.orig,
                    fields: form.fields,
                    suggestions: form.suggestions,
                    uuid: uuid.clone(),
                    book: live_book(),
                    on_cover_applied: move |updated| live_book.set(updated),
                }
                Sidebar {
                    book: live_book(),
                    saving: form.status.saving,
                    on_revert: form.on_revert,
                }
            }

            SaveBar {
                uuid,
                dirty: form.dirty,
                status: form.status,
                on_save: form.on_save,
            }
        }
    }
}

// `build_overrides` (further down) constructs the `MetadataOverrides` from the
// edited fields. Only sets `Some` for fields that differ from the initially
// loaded book (which already has any prior overrides merged in); server-side
// merge then preserves prior overrides on untouched fields.

/// Edited scalar and list fields collected from the form signals before a save.
struct EditedFields<'a> {
    title: &'a str,
    description: &'a str,
    publisher: &'a str,
    published: &'a str,
    language: &'a str,
    series: &'a str,
    series_index: &'a str,
    isbn13: &'a str,
    isbn10: &'a str,
    /// Pre-parsed by the caller (`state::build_on_save`), which rejects a
    /// non-numeric entry client-side before this function ever runs — unlike
    /// every other field here, this one can't be diffed as a raw string
    /// since `MetadataOverrides::print_pages` is `Option<i64>`, not
    /// `Option<String>`.
    print_pages: Option<i64>,
    authors: &'a [String],
    tags: &'a [String],
    genres: &'a [String],
}

/// Build a [`MetadataOverrides`] from the current edit form values.
fn build_overrides(orig: &EbookMetadata, edited: EditedFields<'_>) -> MetadataOverrides {
    let opt = |new: &str, old: Option<&str>| -> Option<String> {
        let old_val = old.unwrap_or("");
        if new != old_val {
            Some(new.to_string())
        } else {
            None
        }
    };

    let orig_authors: Vec<String> = orig.creators.iter().map(|c| c.name.clone()).collect();
    let creators = if edited.authors != orig_authors.as_slice() {
        Some(
            edited
                .authors
                .iter()
                .map(|name| Contributor {
                    name: name.clone(),
                    role: Some("aut".to_string()),
                    file_as: None,
                    id: None,
                })
                .collect(),
        )
    } else {
        None
    };

    let subjects = if edited.tags != orig.subjects.as_slice() {
        Some(edited.tags.to_vec())
    } else {
        None
    };

    let genres = if edited.genres != orig.genres.as_slice() {
        Some(edited.genres.to_vec())
    } else {
        None
    };

    // Unlike the string fields above, `print_pages` has no empty-clears
    // sentinel to fall back on (there is no "empty" `i64`) — a blank form
    // field is `None` from the caller and is treated as "leave the override
    // untouched", not "clear it". Only an actually-changed value is sent.
    let print_pages = edited.print_pages.filter(|&v| Some(v) != orig.print_pages);

    MetadataOverrides {
        title: opt(edited.title, orig.title.as_deref()),
        description: opt(edited.description, orig.description.as_deref()),
        publisher: opt(edited.publisher, orig.publisher.as_deref()),
        published: opt(edited.published, orig.published.as_deref()),
        language: opt(edited.language, orig.language.as_deref()),
        series: opt(edited.series, orig.series.as_deref()),
        series_index: opt(edited.series_index, orig.series_index.as_deref()),
        isbn13: opt(edited.isbn13, orig.isbn13.as_deref()),
        isbn10: opt(edited.isbn10, orig.isbn10.as_deref()),
        creators,
        subjects,
        genres,
        print_pages,
    }
}

#[cfg(test)]
mod tests;
