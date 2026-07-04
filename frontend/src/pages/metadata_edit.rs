//! Single-book metadata edit form at `/books/:uuid/edit`. Two-column
//! layout (form grid + sticky cover/identifiers sidebar) with a sticky
//! save bar showing the dirty field count. Edits persist via
//! [`data::save_overrides`]; the `dirty_count` memo compares each per-field
//! signal to the original merged [`EbookMetadata`] loaded on mount.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use crate::{data, use_server_url, Route};

mod fields;
mod form_grid;
mod header;
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
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::BookDetail { uuid: uuid.clone() }, class: "btn", "Back to book" }
        };
    }
    let Some(b) = book() else {
        return rsx! {
            p { class: "subtitle", "Book not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
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
                }
                Sidebar {
                    book: book.clone(),
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
    authors: &'a [String],
    tags: &'a [String],
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

    MetadataOverrides {
        title: opt(edited.title, orig.title.as_deref()),
        description: opt(edited.description, orig.description.as_deref()),
        publisher: opt(edited.publisher, orig.publisher.as_deref()),
        published: opt(edited.published, orig.published.as_deref()),
        language: opt(edited.language, orig.language.as_deref()),
        series: opt(edited.series, orig.series.as_deref()),
        series_index: opt(edited.series_index, orig.series_index.as_deref()),
        creators,
        subjects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book_with(title: Option<&str>, creators: &[&str], subjects: &[&str]) -> EbookMetadata {
        EbookMetadata {
            title: title.map(String::from),
            creators: creators
                .iter()
                .map(|n| Contributor {
                    name: (*n).to_string(),
                    ..Default::default()
                })
                .collect(),
            subjects: subjects
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            ..Default::default()
        }
    }

    fn edited<'a>(
        title: &'a str,
        publisher: &'a str,
        authors: &'a [String],
        tags: &'a [String],
    ) -> EditedFields<'a> {
        EditedFields {
            title,
            description: "",
            publisher,
            published: "",
            language: "",
            series: "",
            series_index: "",
            authors,
            tags,
        }
    }

    #[test]
    fn build_overrides_no_changes_yields_all_none() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
        let ov = build_overrides(
            &orig,
            edited(
                "Dune",
                "",
                &["Frank Herbert".to_string()],
                &["scifi".to_string()],
            ),
        );
        assert_eq!(ov, MetadataOverrides::default());
    }

    #[test]
    fn build_overrides_sets_only_changed_scalar_fields() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &[]);
        let ov = build_overrides(
            &orig,
            edited("Dune: Messiah", "Ace", &["Frank Herbert".to_string()], &[]),
        );
        assert_eq!(ov.title.as_deref(), Some("Dune: Messiah"));
        assert_eq!(ov.publisher.as_deref(), Some("Ace"));
        assert!(ov.description.is_none());
        assert!(ov.creators.is_none());
        assert!(ov.subjects.is_none());
    }

    #[test]
    fn build_overrides_clearing_a_populated_field_emits_empty_string() {
        // orig.title = "Dune", edited to "" -> the override must carry the
        // empty string so the merge clears it rather than leaving it untouched.
        let orig = book_with(Some("Dune"), &[], &[]);
        let ov = build_overrides(&orig, edited("", "", &[], &[]));
        assert_eq!(ov.title.as_deref(), Some(""));
    }

    #[test]
    fn build_overrides_replaces_full_creator_and_subject_lists() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
        let authors = vec!["Frank Herbert".to_string(), "Brian Herbert".to_string()];
        let tags = vec!["scifi".to_string(), "classic".to_string()];
        let ov = build_overrides(&orig, edited("Dune", "", &authors, &tags));
        let creators = ov.creators.expect("creators should be set");
        assert_eq!(creators.len(), 2);
        assert_eq!(creators[0].name, "Frank Herbert");
        assert_eq!(creators[1].name, "Brian Herbert");
        assert_eq!(creators[0].role.as_deref(), Some("aut"));
        assert_eq!(
            ov.subjects,
            Some(vec!["scifi".to_string(), "classic".to_string()])
        );
    }

    #[test]
    fn build_overrides_unchanged_lists_stay_none() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
        let ov = build_overrides(
            &orig,
            edited(
                "Dune",
                "",
                &["Frank Herbert".to_string()],
                &["scifi".to_string()],
            ),
        );
        assert!(ov.creators.is_none());
        assert!(ov.subjects.is_none());
    }
}
