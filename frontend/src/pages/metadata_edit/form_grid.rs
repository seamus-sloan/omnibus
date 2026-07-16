//! Form grid for the metadata edit page (title row, publisher row, author
//! chips, description, tags, series). Markup-only: all signals are owned
//! by the parent `MetadataEditForm` and passed in via props.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::fields::{label_to_id, MeArea, MeField, MeLabel};
use crate::components::chip_editor::{ChipEditor, ChipEditorOptions, SuggestionItem};
use crate::components::SuggestField;

/// Per-field editable signals threaded through the form rows from `MetadataEditForm`.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct FormFields {
    pub title: Signal<String>,
    pub description: Signal<String>,
    pub publisher: Signal<String>,
    pub published: Signal<String>,
    pub language: Signal<String>,
    pub series: Signal<String>,
    pub series_index: Signal<String>,
    pub isbn13: Signal<String>,
    pub authors: Signal<Vec<String>>,
    pub tags: Signal<Vec<String>>,
    pub sort_by: Signal<String>,
    pub filename: Signal<String>,
}

/// Suggestion pools backing the author + tag chip-editor dropdowns and the
/// series autocomplete field.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct FormSuggestions {
    pub authors: Signal<Vec<SuggestionItem>>,
    pub tags: Signal<Vec<SuggestionItem>>,
    pub series: Signal<Vec<SuggestionItem>>,
}

/// Composed form grid plus the tags + series sections that live in the
/// same left-column container on the page.
#[component]
pub(super) fn FormGrid(
    orig: Signal<EbookMetadata>,
    fields: FormFields,
    suggestions: FormSuggestions,
) -> Element {
    rsx! {
        div { class: "me-form",
            FieldGrid { orig, fields, suggestions }
            TagsSection { tags: fields.tags, tag_suggestions: suggestions.tags }
            SeriesSection {
                orig,
                series: fields.series,
                series_index: fields.series_index,
                series_suggestions: suggestions.series,
            }
        }
    }
}

/// Main field grid: title, sort/filename, publisher, published, language,
/// the author chip row, and the description textarea.
#[component]
fn FieldGrid(
    orig: Signal<EbookMetadata>,
    fields: FormFields,
    suggestions: FormSuggestions,
) -> Element {
    let mut title = fields.title;
    let mut description = fields.description;
    let mut publisher = fields.publisher;
    let mut published = fields.published;
    let mut language = fields.language;
    let mut isbn13 = fields.isbn13;
    let authors = fields.authors;
    let sort_by = fields.sort_by;
    let filename = fields.filename;
    let author_suggestions = suggestions.authors;

    rsx! {
        div { class: "me-field-grid",

            // Title — spans 2 cols, big serif
            MeField {
                label: "Title",
                value: title,
                on_change: move |v: String| title.set(v),
                w: 2,
                big: true,
                serif: true,
                edited: title() != orig().title.clone().unwrap_or_default(),
            }

            // File-as / sort name (mono, read-only for now)
            MeField {
                label: "Sort by",
                value: sort_by,
                on_change: move |_: String| {},
                mono: true,
                locked: true,
                hint: "from file-as",
            }

            // Filename (mono, read-only)
            MeField {
                label: "Filename",
                value: filename,
                on_change: move |_: String| {},
                mono: true,
                locked: true,
            }

            // Publisher
            MeField {
                label: "Publisher",
                value: publisher,
                on_change: move |v: String| publisher.set(v),
                w: 2,
                edited: publisher() != orig().publisher.clone().unwrap_or_default(),
            }

            // Published date
            MeField {
                label: "Published",
                value: published,
                on_change: move |v: String| published.set(v),
                mono: true,
                edited: published() != orig().published.clone().unwrap_or_default(),
            }

            // Language
            MeField {
                label: "Language",
                value: language,
                on_change: move |v: String| language.set(v),
                edited: language() != orig().language.clone().unwrap_or_default(),
            }

            // ISBN-13 — exactly 13 digits, enforced server-side on save
            // (`MetadataOverrides::validate`); an empty value clears it.
            MeField {
                label: "ISBN-13",
                value: isbn13,
                on_change: move |v: String| isbn13.set(v),
                mono: true,
                edited: isbn13() != orig().isbn13.clone().unwrap_or_default(),
                hint: "13 digits",
                placeholder: "e.g. 9780134685991",
            }

            // Authors — chip row spanning 4 cols
            div { class: "me-field-full",
                MeLabel {
                    text: "Author(s)",
                    edited: {
                        let orig_authors: Vec<String> = orig().creators.iter().map(|c| c.name.clone()).collect();
                        authors() != orig_authors
                    },
                    hint: "primary author first",
                }
                div { class: "me-chip-row",
                    ChipEditor {
                        values: authors,
                        on_change: move |_| {},
                        suggestions: author_suggestions,
                        options: ChipEditorOptions {
                            placeholder: "+ add author\u{2026}".to_string(),
                            show_avatar: true,
                            aria_remove_prefix: "Remove".to_string(),
                            testid_prefix: "me-authors".to_string(),
                            ..ChipEditorOptions::default()
                        },
                    }
                }
            }

            // Description — textarea spanning 2 cols
            MeArea {
                label: "Description",
                value: description,
                on_change: move |v: String| description.set(v),
                rows: 5,
                edited: description() != orig().description.clone().unwrap_or_default(),
                hint: "plain text or HTML",
            }
        }
    }
}

/// Divider + tag chip editor for the metadata form's tags section.
#[component]
fn TagsSection(tags: Signal<Vec<String>>, tag_suggestions: Signal<Vec<SuggestionItem>>) -> Element {
    rsx! {
        div { class: "divider" }
        div { class: "me-tags-header",
            div { class: "label", "Tags" }
        }
        div { class: "me-tag-chips",
            ChipEditor {
                values: tags,
                on_change: move |_| {},
                suggestions: tag_suggestions,
                options: ChipEditorOptions {
                    placeholder: "+ add tag\u{2026}".to_string(),
                    show_avatar: false,
                    input_class: "me-tag-input".to_string(),
                    aria_remove_prefix: "Remove tag".to_string(),
                    testid_prefix: "me-tags".to_string(),
                    ..ChipEditorOptions::default()
                },
            }
        }
    }
}

/// Divider + series name (autocomplete against the existing series pool) and
/// book-number fields for the metadata form. Series is a single value (a
/// book has one series), so unlike the author/tag rows this renders
/// [`SuggestField`] instead of [`ChipEditor`] — same dropdown, no chip list.
#[component]
fn SeriesSection(
    orig: Signal<EbookMetadata>,
    series: Signal<String>,
    series_index: Signal<String>,
    series_suggestions: Signal<Vec<SuggestionItem>>,
) -> Element {
    let mut series_index = series_index;
    let series_edited = series() != orig().series.clone().unwrap_or_default();
    let field_id = label_to_id("Series");
    let input_class = if series_edited {
        "me-input me-input-edited"
    } else {
        "me-input"
    };
    rsx! {
        div { class: "divider" }
        div { class: "label", "Series & position" }
        div { class: "me-series-grid",
            div { class: "me-field",
                MeLabel { text: "Series", edited: series_edited, target: field_id.clone() }
                SuggestField {
                    id: field_id,
                    class: input_class,
                    value: series,
                    suggestions: series_suggestions,
                    placeholder: "not part of a series",
                    testid_prefix: "me-series",
                }
            }
            MeField {
                label: "Book #",
                value: series_index,
                on_change: move |v: String| series_index.set(v),
                mono: true,
                placeholder: "\u{2014}",
                edited: series_index() != orig().series_index.clone().unwrap_or_default(),
            }
        }
    }
}
