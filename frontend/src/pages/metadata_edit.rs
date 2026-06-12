//! Single-book metadata edit form at `/books/:uuid/edit`. Two-column
//! layout (form grid + sticky cover/identifiers sidebar) with a sticky
//! save bar showing the dirty field count. Edits persist via
//! [`data::save_overrides`]; the `dirty_count` memo compares each per-field
//! signal to the original merged [`EbookMetadata`] loaded on mount.

use dioxus::prelude::*;
use dioxus_router::{navigator, Link};
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use crate::components::chip_editor::{collect_suggestions, SuggestionItem};
use crate::{data, use_server_url, Route};

mod fields;
mod form_grid;
mod header;
mod save_bar;
mod sidebar;

use form_grid::{FormFields, FormGrid, FormSuggestions};
use header::{Breadcrumb, PageHeader};
use save_bar::{DirtyState, SaveBar, SaveStatus};
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

// ---------------------------------------------------------------------------
// Edit form — the loaded-book case. Owns all the per-field signals and the
// save/discard logic; renders composition over the page-local
// sub-components in `fields`/`form_grid`/`header`/`sidebar`/`save_bar`.
// ---------------------------------------------------------------------------

#[component]
fn MetadataEditForm(book: EbookMetadata, uuid: String) -> Element {
    let server_url = use_server_url();

    // Original values — frozen snapshot for dirty comparison.
    let orig = use_signal(|| book.clone());

    // Per-field editable signals.
    let title = use_signal(|| book.title.clone().unwrap_or_default());
    let description = use_signal(|| book.description.clone().unwrap_or_default());
    let publisher = use_signal(|| book.publisher.clone().unwrap_or_default());
    let published = use_signal(|| book.published.clone().unwrap_or_default());
    let language = use_signal(|| book.language.clone().unwrap_or_default());
    let series = use_signal(|| book.series.clone().unwrap_or_default());
    let series_index = use_signal(|| book.series_index.clone().unwrap_or_default());

    // Authors as a signal of Vec<String> (names only for v1).
    let authors = use_signal(|| {
        book.creators
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
    });

    // Tags (subjects) as a signal of Vec<String>.
    let tags = use_signal(|| book.subjects.clone());

    // Read-only field signals — hoisted here so `use_signal` isn't called
    // inside the `rsx!` body on every render.
    let sort_by = use_signal(|| {
        book.creators
            .first()
            .map(|c| c.file_as.clone().unwrap_or_else(|| c.name.clone()))
            .unwrap_or_default()
    });
    let filename = use_signal(|| book.filename.clone());

    // Suggestion pools for the ChipEditor dropdowns. Each item carries
    // the book-count the dropdown row renders next to the name. Empty
    // until the mount-time fetches resolve; an empty signal renders no
    // dropdown.
    let mut author_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut tag_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    {
        let url = server_url.clone();
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
            });
        });
    }

    // Save / error state.
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);

    // Dirty-field tracking.
    let dirty_fields = use_memo(move || {
        let o = orig();
        let mut fields: Vec<&str> = Vec::new();
        if title() != o.title.clone().unwrap_or_default() {
            fields.push("Title");
        }
        if description() != o.description.clone().unwrap_or_default() {
            fields.push("Description");
        }
        if publisher() != o.publisher.clone().unwrap_or_default() {
            fields.push("Publisher");
        }
        if published() != o.published.clone().unwrap_or_default() {
            fields.push("Published");
        }
        if language() != o.language.clone().unwrap_or_default() {
            fields.push("Language");
        }
        if series() != o.series.clone().unwrap_or_default() {
            fields.push("Series");
        }
        if series_index() != o.series_index.clone().unwrap_or_default() {
            fields.push("Book #");
        }
        let orig_authors: Vec<String> = o.creators.iter().map(|c| c.name.clone()).collect();
        if authors() != orig_authors {
            fields.push("Authors");
        }
        if tags() != o.subjects {
            fields.push("Tags");
        }
        fields
    });

    let dirty_count = use_memo(move || dirty_fields().len());

    let display_title = book.title.clone().unwrap_or_else(|| book.filename.clone());
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

    // Revert handler — deletes overrides and navigates back to the book
    // detail page on success.
    let on_revert = {
        let url = server_url.clone();
        let uuid = uuid.clone();
        move |_| {
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
        }
    };

    // Save handler — builds the diff and POSTs to the overrides endpoint,
    // then navigates back to the book detail page on success.
    let on_save = {
        let url = server_url.clone();
        let uuid = uuid.clone();
        move |_| {
            let url = url.clone();
            let uuid = uuid.clone();
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
                        authors: &authors(),
                        tags: &tags(),
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
        }
    };

    rsx! {
        div { class: "me-root", style: "{accent_style}",
            Breadcrumb {
                uuid: uuid.clone(),
                display_title: display_title.clone(),
                primary_author: primary_author.clone(),
                primary_author_id,
            }
            PageHeader {
                display_title,
                primary_author,
            }

            div { class: "me-layout",
                FormGrid {
                    orig,
                    fields: FormFields {
                        title,
                        description,
                        publisher,
                        published,
                        language,
                        series,
                        series_index,
                        authors,
                        tags,
                        sort_by,
                        filename,
                    },
                    suggestions: FormSuggestions {
                        authors: author_suggestions,
                        tags: tag_suggestions,
                    },
                }
                Sidebar {
                    book: book.clone(),
                    saving,
                    on_revert,
                }
            }

            SaveBar {
                uuid,
                dirty: DirtyState {
                    fields: dirty_fields,
                    count: dirty_count,
                },
                status: SaveStatus {
                    saving,
                    error: save_error,
                },
                on_save,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Build the MetadataOverrides from the edited fields. Only sets `Some` for
// fields that differ from the initially loaded book (which already has any
// prior overrides merged in). Server-side merge ensures prior overrides on
// untouched fields are preserved.
// ---------------------------------------------------------------------------

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
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
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
