//! Bulk-edit surface for the landing table: the floating action bar shown
//! while rows are checked, and the modal that applies one
//! `BulkMetadataEdit` to every selected book via `rpc_bulk_save_overrides`.

use dioxus::prelude::*;
use omnibus_shared::{BulkMetadataEdit, EbookMetadata};

use crate::components::chip_editor::{ChipEditor, ChipEditorOptions, SuggestionItem};
use crate::components::ConfirmModal;
use crate::{data, use_server_url};

/// Floating action bar shown while at least one table row is selected.
#[component]
pub(super) fn BulkEditBar(
    count: usize,
    on_edit: EventHandler<()>,
    on_clear: EventHandler<()>,
) -> Element {
    let noun = if count == 1 { "book" } else { "books" };
    rsx! {
        div { class: "bulk-edit-bar", "data-testid": "bulk-edit-bar",
            span { class: "bulk-edit-count", "{count} {noun} selected" }
            button {
                r#type: "button",
                class: "btn shelf-btn-primary",
                "data-testid": "bulk-edit-open",
                onclick: move |_| on_edit.call(()),
                "Edit"
            }
            button {
                r#type: "button",
                class: "btn shelf-btn-ghost",
                "data-testid": "bulk-edit-clear",
                onclick: move |_| on_clear.call(()),
                "Clear selection"
            }
        }
    }
}

/// The library-wide suggestion pools the modal's chip editors read, grouped
/// to keep [`BulkEditModal`] under the 5-prop guideline. The remove-side
/// pools are derived from the selection inside the modal, not passed in.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct BulkEditSuggestions {
    pub author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub genre_suggestions: ReadSignal<Vec<SuggestionItem>>,
}

/// The bulk-edit modal. Blank scalar fields leave every book unchanged;
/// authors replace each book's full list; tags and genres are add/remove
/// deltas.
/// On success fires `on_saved` with the server's merged metadata for every
/// edited book so the caller can patch its lists without a refetch.
#[component]
pub(super) fn BulkEditModal(
    uuids: Vec<String>,
    selected_books: Vec<EbookMetadata>,
    suggestions: BulkEditSuggestions,
    on_close: EventHandler<()>,
    on_saved: EventHandler<Vec<EbookMetadata>>,
) -> Element {
    let server_url = use_server_url();
    let fields = BulkEditFieldSignals {
        authors: use_signal(Vec::<String>::new),
        series: use_signal(String::new),
        publisher: use_signal(String::new),
        language: use_signal(String::new),
        add_tags: use_signal(Vec::<String>::new),
        remove_tags: use_signal(Vec::<String>::new),
        add_genres: use_signal(Vec::<String>::new),
        remove_genres: use_signal(Vec::<String>::new),
    };
    let busy = use_signal(|| false);
    let error = use_signal(|| None::<String>);

    // Remove-side candidates: the union of the selected books' current
    // tags/genres, with per-value counts — removing one no book carries is
    // meaningless. Annotated because a bare `.into()` in the rsx props is
    // ambiguous between the dioxus_core and dioxus_stores `SuperInto` impls.
    let books_for_genres = selected_books.clone();
    let removable = use_memo(move || removable_tags(&selected_books));
    let removable_genre = use_memo(move || removable_genres(&books_for_genres));
    let pools = BulkEditPools {
        authors: suggestions.author_suggestions,
        tags: suggestions.tag_suggestions,
        genres: suggestions.genre_suggestions,
        removable_tags: removable.into(),
        removable_genres: removable_genre.into(),
    };

    let nothing_to_apply = fields.current_edit().is_empty();
    let count = uuids.len();
    let noun = if count == 1 { "book" } else { "books" };
    let on_submit = build_bulk_edit_submit(server_url, uuids, fields, busy, error, on_saved);

    rsx! {
        ConfirmModal {
            testid: "bulk-edit-modal",
            aria_label: "Bulk edit {count} {noun}",
            dialog_class: "bulk-edit-dialog",
            busy: busy(),
            on_dismiss: move |_| on_close.call(()),
            h3 { class: "bulk-edit-title", "Edit {count} {noun}" }
            p { class: "bulk-edit-hint", "Blank fields are left unchanged on every book." }
            BulkEditFields { fields, pools }
            if let Some(msg) = error() {
                p { class: "bulk-edit-error", role: "alert", "data-testid": "bulk-edit-error", "{msg}" }
            }
            div { class: "shelf-modal-foot",
                button {
                    r#type: "button",
                    class: "btn shelf-btn-ghost",
                    "data-testid": "bulk-edit-cancel",
                    disabled: busy(),
                    onclick: move |_| on_close.call(()),
                    "Cancel"
                }
                button {
                    r#type: "button",
                    class: "btn shelf-btn-primary",
                    "data-testid": "bulk-edit-submit",
                    disabled: busy() || nothing_to_apply,
                    onclick: on_submit,
                    if busy() { "Applying\u{2026}" } else { "Apply to {count} {noun}" }
                }
            }
        }
    }
}

/// Builds the bulk-edit submit handler: snapshots and validates the current
/// field state, then saves it and reports the merged metadata back through
/// `error`/`busy`/`on_saved`. Mirrors
/// `create_shelf_modal::build_on_submit`.
fn build_bulk_edit_submit(
    server_url: String,
    uuids: Vec<String>,
    fields: BulkEditFieldSignals,
    mut busy: Signal<bool>,
    mut error: Signal<Option<String>>,
    on_saved: EventHandler<Vec<EbookMetadata>>,
) -> EventHandler<MouseEvent> {
    EventHandler::new(move |_| {
        if busy() {
            return;
        }
        let edit = fields.current_edit();
        if edit.is_empty() {
            return;
        }
        if let Err(msg) = edit.validate() {
            error.set(Some(msg));
            return;
        }
        let url = server_url.clone();
        let uuids = uuids.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::bulk_save_overrides(&url, uuids, &edit).await {
                Ok(updated) => on_saved.call(updated),
                Err(e) => error.set(Some(format!("Save failed: {e}"))),
            }
            busy.set(false);
        });
    })
}

/// The modal's field signals, grouped so [`BulkEditModal`] and
/// [`BulkEditFields`] share one handle bundle (all `Signal`s are `Copy`).
#[derive(Clone, Copy, PartialEq)]
struct BulkEditFieldSignals {
    authors: Signal<Vec<String>>,
    series: Signal<String>,
    publisher: Signal<String>,
    language: Signal<String>,
    add_tags: Signal<Vec<String>>,
    remove_tags: Signal<Vec<String>>,
    add_genres: Signal<Vec<String>>,
    remove_genres: Signal<Vec<String>>,
}

impl BulkEditFieldSignals {
    /// Snapshot the current field state as the wire payload.
    fn current_edit(&self) -> BulkMetadataEdit {
        build_edit(BulkEditValues {
            authors: &self.authors.read(),
            series: &self.series.read(),
            publisher: &self.publisher.read(),
            language: &self.language.read(),
            add_tags: &self.add_tags.read(),
            remove_tags: &self.remove_tags.read(),
            add_genres: &self.add_genres.read(),
            remove_genres: &self.remove_genres.read(),
        })
    }
}

/// The four suggestion pools the modal's chip editors read. Grouped so
/// [`BulkEditFields`] stays under the prop cap; `removable_tags` /
/// `removable_genres` are the *selection-scoped* pools (only values some
/// selected book actually carries), the other two are library-wide.
#[derive(Clone, Copy, PartialEq)]
struct BulkEditPools {
    authors: ReadSignal<Vec<SuggestionItem>>,
    tags: ReadSignal<Vec<SuggestionItem>>,
    genres: ReadSignal<Vec<SuggestionItem>>,
    removable_tags: ReadSignal<Vec<SuggestionItem>>,
    removable_genres: ReadSignal<Vec<SuggestionItem>>,
}

/// The labeled field rows inside the bulk-edit modal body.
#[component]
fn BulkEditFields(fields: BulkEditFieldSignals, pools: BulkEditPools) -> Element {
    rsx! {
        div { class: "bulk-edit-fields",
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Authors" }
                ChipEditor {
                    values: fields.authors,
                    on_change: move |_: Vec<String>| {},
                    suggestions: pools.authors,
                    options: ChipEditorOptions {
                        placeholder: "Replace authors\u{2026}".to_string(),
                        show_avatar: true,
                        testid_prefix: "bulk-authors".to_string(),
                        dropdown_header: "ADD AUTHOR".to_string(),
                        ..Default::default()
                    },
                }
            }
            BulkTextField {
                id: "bulk-series",
                label: "Series",
                placeholder: "Series name",
                value: fields.series,
            }
            BulkTextField {
                id: "bulk-publisher",
                label: "Publisher",
                placeholder: "Publisher",
                value: fields.publisher,
            }
            BulkTextField {
                id: "bulk-language",
                label: "Language",
                placeholder: "en",
                value: fields.language,
            }
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Add tags" }
                ChipEditor {
                    values: fields.add_tags,
                    on_change: move |_: Vec<String>| {},
                    suggestions: pools.tags,
                    options: ChipEditorOptions {
                        placeholder: "Add a tag to every book\u{2026}".to_string(),
                        input_class: "me-tag-input".to_string(),
                        aria_remove_prefix: "Remove tag".to_string(),
                        testid_prefix: "bulk-add-tags".to_string(),
                        dropdown_header: "ADD TAG".to_string(),
                        ..Default::default()
                    },
                }
            }
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Remove tags" }
                ChipEditor {
                    values: fields.remove_tags,
                    on_change: move |_: Vec<String>| {},
                    suggestions: pools.removable_tags,
                    options: ChipEditorOptions {
                        placeholder: "Remove a tag where present\u{2026}".to_string(),
                        input_class: "me-tag-input".to_string(),
                        aria_remove_prefix: "Remove tag".to_string(),
                        testid_prefix: "bulk-remove-tags".to_string(),
                        dropdown_header: "REMOVE TAG".to_string(),
                        ..Default::default()
                    },
                }
            }
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Add genres" }
                ChipEditor {
                    values: fields.add_genres,
                    on_change: move |_: Vec<String>| {},
                    suggestions: pools.genres,
                    options: ChipEditorOptions {
                        placeholder: "Add a genre to every book\u{2026}".to_string(),
                        input_class: "me-tag-input".to_string(),
                        aria_remove_prefix: "Remove genre".to_string(),
                        testid_prefix: "bulk-add-genres".to_string(),
                        dropdown_header: "ADD GENRE".to_string(),
                        ..Default::default()
                    },
                }
            }
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Remove genres" }
                ChipEditor {
                    values: fields.remove_genres,
                    on_change: move |_: Vec<String>| {},
                    suggestions: pools.removable_genres,
                    options: ChipEditorOptions {
                        placeholder: "Remove a genre where present\u{2026}".to_string(),
                        input_class: "me-tag-input".to_string(),
                        aria_remove_prefix: "Remove genre".to_string(),
                        testid_prefix: "bulk-remove-genres".to_string(),
                        dropdown_header: "REMOVE GENRE".to_string(),
                        ..Default::default()
                    },
                }
            }
        }
    }
}

/// One labeled scalar text input in the bulk-edit modal. The `data-testid`
/// is derived as `{id}-input` (part of the Playwright contract).
#[component]
fn BulkTextField(
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    value: Signal<String>,
) -> Element {
    let mut value = value;
    rsx! {
        div { class: "bulk-edit-field",
            label { class: "bulk-edit-label", r#for: "{id}", "{label}" }
            input {
                id: "{id}",
                "data-testid": "{id}-input",
                class: "bulk-edit-input",
                r#type: "text",
                placeholder: "{placeholder}",
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
        }
    }
}

/// The modal's field values at submit time, borrowed from the signals.
/// A struct rather than eight positional args so a caller can't transpose
/// two same-typed lists silently.
struct BulkEditValues<'a> {
    authors: &'a [String],
    series: &'a str,
    publisher: &'a str,
    language: &'a str,
    add_tags: &'a [String],
    remove_tags: &'a [String],
    add_genres: &'a [String],
    remove_genres: &'a [String],
}

/// Assemble the wire payload from the modal's field state. Trimmed-empty
/// scalars and an empty authors chip list map to `None` (= unchanged) —
/// bulk edit has no clear affordance in v1.
fn build_edit(v: BulkEditValues<'_>) -> BulkMetadataEdit {
    let scalar = |value: &str| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    BulkMetadataEdit {
        authors: (!v.authors.is_empty()).then(|| v.authors.to_vec()),
        series: scalar(v.series),
        publisher: scalar(v.publisher),
        language: scalar(v.language),
        add_tags: v.add_tags.to_vec(),
        remove_tags: v.remove_tags.to_vec(),
        add_genres: v.add_genres.to_vec(),
        remove_genres: v.remove_genres.to_vec(),
    }
}

/// Union of the selected books' current genres with per-genre occurrence
/// counts — the suggestion pool for "Remove genres". Same shape as
/// [`removable_tags`], over a different list.
fn removable_genres(books: &[EbookMetadata]) -> Vec<SuggestionItem> {
    rank_by_frequency(books.iter().flat_map(|b| b.genres.iter()))
}

/// Union of the selected books' current tags with per-tag occurrence counts,
/// sorted by frequency then name — the suggestion pool for "Remove tags".
fn removable_tags(books: &[EbookMetadata]) -> Vec<SuggestionItem> {
    rank_by_frequency(books.iter().flat_map(|b| b.subjects.iter()))
}

/// Count occurrences of each distinct value and rank them by frequency
/// descending, then name ascending.
fn rank_by_frequency<'a>(values: impl Iterator<Item = &'a String>) -> Vec<SuggestionItem> {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for value in values {
        *counts.entry(value.as_str()).or_default() += 1;
    }
    let mut items: Vec<SuggestionItem> = counts
        .into_iter()
        .map(|(name, count)| SuggestionItem::new(name, count))
        .collect();
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A [`BulkEditValues`] with everything blank — tests override just the
    /// fields they exercise.
    fn empty_values<'a>() -> BulkEditValues<'a> {
        BulkEditValues {
            authors: &[],
            series: "",
            publisher: "",
            language: "",
            add_tags: &[],
            remove_tags: &[],
            add_genres: &[],
            remove_genres: &[],
        }
    }

    #[test]
    fn build_edit_maps_blank_scalars_and_empty_authors_to_unchanged() {
        let edit = build_edit(BulkEditValues {
            series: "  ",
            language: "\t",
            ..empty_values()
        });
        assert!(edit.is_empty());
    }

    #[test]
    fn build_edit_trims_scalars_and_carries_chip_lists() {
        let authors = strings(&["Ada Lovelace"]);
        let add_tags = strings(&["fantasy"]);
        let remove_tags = strings(&["scifi"]);
        let add_genres = strings(&["Fantasy"]);
        let remove_genres = strings(&["Sci-Fi"]);
        let edit = build_edit(BulkEditValues {
            authors: &authors,
            series: " Earthsea ",
            publisher: "Harcourt",
            language: "en",
            add_tags: &add_tags,
            remove_tags: &remove_tags,
            add_genres: &add_genres,
            remove_genres: &remove_genres,
        });
        assert_eq!(edit.authors, Some(strings(&["Ada Lovelace"])));
        assert_eq!(edit.series.as_deref(), Some("Earthsea"));
        assert_eq!(edit.publisher.as_deref(), Some("Harcourt"));
        assert_eq!(edit.language.as_deref(), Some("en"));
        assert_eq!(edit.add_tags, strings(&["fantasy"]));
        assert_eq!(edit.remove_tags, strings(&["scifi"]));
        assert_eq!(edit.add_genres, strings(&["Fantasy"]));
        assert_eq!(edit.remove_genres, strings(&["Sci-Fi"]));
    }

    #[test]
    fn build_edit_treats_a_genre_only_edit_as_something_to_apply() {
        let add_genres = strings(&["Horror"]);
        let edit = build_edit(BulkEditValues {
            add_genres: &add_genres,
            ..empty_values()
        });
        assert!(!edit.is_empty(), "Apply must not stay disabled");
        assert!(edit.add_tags.is_empty(), "genres do not leak into tags");
    }

    #[test]
    fn removable_genres_counts_across_books_and_ignores_tags() {
        let a = EbookMetadata {
            subjects: strings(&["scifi"]),
            genres: strings(&["Horror", "Mystery"]),
            ..Default::default()
        };
        let b = EbookMetadata {
            genres: strings(&["Horror"]),
            ..Default::default()
        };
        let named: Vec<(String, usize)> = removable_genres(&[a, b])
            .into_iter()
            .map(|i| (i.name, i.count))
            .collect();
        assert_eq!(
            named,
            vec![("Horror".to_string(), 2), ("Mystery".to_string(), 1)],
            "tags must not appear in the removable-genre pool"
        );
    }

    #[test]
    fn removable_tags_counts_across_books_and_sorts_by_frequency_then_name() {
        let a = EbookMetadata {
            subjects: strings(&["scifi", "classic"]),
            ..Default::default()
        };
        let b = EbookMetadata {
            subjects: strings(&["scifi", "adventure"]),
            ..Default::default()
        };
        let items = removable_tags(&[a, b]);
        let named: Vec<(String, usize)> = items.into_iter().map(|i| (i.name, i.count)).collect();
        assert_eq!(
            named,
            vec![
                ("scifi".to_string(), 2),
                ("adventure".to_string(), 1),
                ("classic".to_string(), 1),
            ]
        );
    }
}
