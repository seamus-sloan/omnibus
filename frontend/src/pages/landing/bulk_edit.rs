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

/// The bulk-edit modal. Blank scalar fields leave every book unchanged;
/// authors replace each book's full list; tags are add/remove deltas.
/// On success fires `on_saved` with the server's merged metadata for every
/// edited book so the caller can patch its lists without a refetch.
#[component]
pub(super) fn BulkEditModal(
    uuids: Vec<String>,
    selected_books: Vec<EbookMetadata>,
    author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
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
    };
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // Remove-tag candidates: the union of the selected books' current tags,
    // with per-tag counts — removing a tag no book carries is meaningless.
    // Annotated because a bare `.into()` in the rsx props is ambiguous
    // between the dioxus_core and dioxus_stores `SuperInto` impls.
    let removable = use_memo(move || removable_tags(&selected_books));
    let removable_pool: ReadSignal<Vec<SuggestionItem>> = removable.into();

    let nothing_to_apply = fields.current_edit().is_empty();
    let count = uuids.len();
    let noun = if count == 1 { "book" } else { "books" };

    let on_submit = move |_| {
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
    };

    rsx! {
        ConfirmModal {
            testid: "bulk-edit-modal",
            aria_label: "Bulk edit {count} {noun}",
            dialog_class: "bulk-edit-dialog",
            busy: busy(),
            on_dismiss: move |_| on_close.call(()),
            h3 { class: "bulk-edit-title", "Edit {count} {noun}" }
            p { class: "bulk-edit-hint", "Blank fields are left unchanged on every book." }
            BulkEditFields {
                fields,
                author_suggestions,
                tag_suggestions,
                removable_pool,
            }
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

/// The modal's six field signals, grouped so [`BulkEditModal`] and
/// [`BulkEditFields`] share one handle bundle (all `Signal`s are `Copy`).
#[derive(Clone, Copy, PartialEq)]
struct BulkEditFieldSignals {
    authors: Signal<Vec<String>>,
    series: Signal<String>,
    publisher: Signal<String>,
    language: Signal<String>,
    add_tags: Signal<Vec<String>>,
    remove_tags: Signal<Vec<String>>,
}

impl BulkEditFieldSignals {
    /// Snapshot the current field state as the wire payload.
    fn current_edit(&self) -> BulkMetadataEdit {
        build_edit(
            &self.authors.read(),
            &self.series.read(),
            &self.publisher.read(),
            &self.language.read(),
            &self.add_tags.read(),
            &self.remove_tags.read(),
        )
    }
}

/// The six labeled field rows inside the bulk-edit modal body.
#[component]
fn BulkEditFields(
    fields: BulkEditFieldSignals,
    author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
    removable_pool: ReadSignal<Vec<SuggestionItem>>,
) -> Element {
    rsx! {
        div { class: "bulk-edit-fields",
            div { class: "bulk-edit-field",
                span { class: "bulk-edit-label", "Authors" }
                ChipEditor {
                    values: fields.authors,
                    on_change: move |_: Vec<String>| {},
                    suggestions: author_suggestions,
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
                    suggestions: tag_suggestions,
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
                    suggestions: removable_pool,
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

/// Assemble the wire payload from the modal's field state. Trimmed-empty
/// scalars and an empty authors chip list map to `None` (= unchanged) —
/// bulk edit has no clear affordance in v1.
fn build_edit(
    authors: &[String],
    series: &str,
    publisher: &str,
    language: &str,
    add_tags: &[String],
    remove_tags: &[String],
) -> BulkMetadataEdit {
    let scalar = |v: &str| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    BulkMetadataEdit {
        authors: (!authors.is_empty()).then(|| authors.to_vec()),
        series: scalar(series),
        publisher: scalar(publisher),
        language: scalar(language),
        add_tags: add_tags.to_vec(),
        remove_tags: remove_tags.to_vec(),
    }
}

/// Union of the selected books' current tags with per-tag occurrence counts,
/// sorted by frequency then name — the suggestion pool for "Remove tags".
fn removable_tags(books: &[EbookMetadata]) -> Vec<SuggestionItem> {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for book in books {
        for tag in &book.subjects {
            *counts.entry(tag.as_str()).or_default() += 1;
        }
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

    #[test]
    fn build_edit_maps_blank_scalars_and_empty_authors_to_unchanged() {
        let edit = build_edit(&[], "  ", "", "\t", &[], &[]);
        assert!(edit.is_empty());
    }

    #[test]
    fn build_edit_trims_scalars_and_carries_chip_lists() {
        let edit = build_edit(
            &strings(&["Ada Lovelace"]),
            " Earthsea ",
            "Harcourt",
            "en",
            &strings(&["fantasy"]),
            &strings(&["scifi"]),
        );
        assert_eq!(edit.authors, Some(strings(&["Ada Lovelace"])));
        assert_eq!(edit.series.as_deref(), Some("Earthsea"));
        assert_eq!(edit.publisher.as_deref(), Some("Harcourt"));
        assert_eq!(edit.language.as_deref(), Some("en"));
        assert_eq!(edit.add_tags, strings(&["fantasy"]));
        assert_eq!(edit.remove_tags, strings(&["scifi"]));
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
