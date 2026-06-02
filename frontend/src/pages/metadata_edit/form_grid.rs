//! Form grid for the metadata edit page: title/sort-by/filename row,
//! publisher/published/language row, author chips, description textarea,
//! tags chip row, and the series sub-grid.
//!
//! All signals are owned by the parent `MetadataEditForm` and passed in;
//! this module is markup-only.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::fields::{MeArea, MeField, MeLabel};
use crate::components::chip_editor::{ChipEditor, SuggestionItem};

/// Composed form grid plus the tags + series sections that live in the
/// same left-column container on the page.
#[component]
#[allow(clippy::too_many_arguments)]
pub(super) fn FormGrid(
    orig: Signal<EbookMetadata>,
    title: Signal<String>,
    description: Signal<String>,
    publisher: Signal<String>,
    published: Signal<String>,
    language: Signal<String>,
    series: Signal<String>,
    series_index: Signal<String>,
    authors: Signal<Vec<String>>,
    tags: Signal<Vec<String>>,
    sort_by: Signal<String>,
    filename: Signal<String>,
    author_suggestions: Signal<Vec<SuggestionItem>>,
    tag_suggestions: Signal<Vec<SuggestionItem>>,
) -> Element {
    let mut title = title;
    let mut description = description;
    let mut publisher = publisher;
    let mut published = published;
    let mut language = language;
    let mut series = series;
    let mut series_index = series_index;

    rsx! {
        div { class: "me-form",
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
                            placeholder: "+ add author\u{2026}".to_string(),
                            on_change: move |_| {},
                            suggestions: author_suggestions,
                            show_avatar: true,
                            aria_remove_prefix: "Remove".to_string(),
                            testid_prefix: "me-authors".to_string(),
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

            // ── Tags section ───────────────────────────────
            div { class: "divider" }
            div { class: "me-tags-header",
                div { class: "label", "Tags" }
            }
            div { class: "me-tag-chips",
                ChipEditor {
                    values: tags,
                    placeholder: "+ add tag\u{2026}".to_string(),
                    on_change: move |_| {},
                    suggestions: tag_suggestions,
                    show_avatar: false,
                    input_class: "me-tag-input".to_string(),
                    aria_remove_prefix: "Remove tag".to_string(),
                    testid_prefix: "me-tags".to_string(),
                }
            }

            // ── Series section ─────────────────────────────
            div { class: "divider" }
            div { class: "label", "Series & position" }
            div { class: "me-series-grid",
                MeField {
                    label: "Series",
                    value: series,
                    on_change: move |v: String| series.set(v),
                    placeholder: "not part of a series",
                    edited: series() != orig().series.clone().unwrap_or_default(),
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
}
