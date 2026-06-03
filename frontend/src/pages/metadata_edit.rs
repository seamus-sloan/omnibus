//! Single-book metadata edit form at `/books/:uuid/edit`. Two-column
//! layout (form grid + sticky cover/identifiers sidebar) with a sticky
//! save bar showing the dirty field count. Edits persist via
//! [`data::save_overrides`]; the `dirty_count` memo compares each per-field
//! signal to the original merged [`EbookMetadata`] loaded on mount.

use dioxus::prelude::*;
use dioxus_router::{navigator, Link};
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use crate::components::atrium::Cover;
use crate::components::chip_editor::{collect_suggestions, ChipEditor, SuggestionItem};
use crate::{data, use_server_url, Route};

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
// save/discard logic.
// ---------------------------------------------------------------------------

#[component]
fn MetadataEditForm(book: EbookMetadata, uuid: String) -> Element {
    let server_url = use_server_url();

    // Original values — frozen snapshot for dirty comparison.
    let orig = use_signal(|| book.clone());

    // Per-field editable signals.
    let mut title = use_signal(|| book.title.clone().unwrap_or_default());
    let mut description = use_signal(|| book.description.clone().unwrap_or_default());
    let mut publisher = use_signal(|| book.publisher.clone().unwrap_or_default());
    let mut published = use_signal(|| book.published.clone().unwrap_or_default());
    let mut language = use_signal(|| book.language.clone().unwrap_or_default());
    let mut series = use_signal(|| book.series.clone().unwrap_or_default());
    let mut series_index = use_signal(|| book.series_index.clone().unwrap_or_default());

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

    rsx! {
        div { class: "me-root", style: "{accent_style}",

            // ── Breadcrumb ─────────────────────────────────────────
            nav { class: "bd-crumb me-crumb", "aria-label": "breadcrumb",
                Link { to: Route::Landing {}, class: "bd-crumb-home", "Home" }
                span { class: "bd-crumb-sep", "\u{203a}" }
                if !primary_author.is_empty() {
                    if let Some(author_id) = primary_author_id {
                        Link {
                            to: Route::AuthorDetail { id: author_id },
                            class: "bd-crumb-step",
                            "data-testid": "me-crumb-author",
                            "{primary_author}"
                        }
                    } else {
                        span {
                            class: "bd-crumb-step",
                            "data-testid": "me-crumb-author",
                            "{primary_author}"
                        }
                    }
                    span { class: "bd-crumb-sep", "\u{203a}" }
                }
                Link { to: Route::BookDetail { uuid: uuid.clone() }, class: "bd-crumb-step", "{display_title}" }
                span { class: "bd-crumb-sep", "\u{203a}" }
                span { class: "bd-crumb-curr", "Edit metadata" }
            }

            // ── Page header ────────────────────────────────────────
            div { class: "me-page-header",
                div {
                    div {
                        class: "label",
                        "data-testid": "me-page-title-label",
                        "Edit metadata"
                    }
                    h2 { class: "me-page-title",
                        span { class: "me-page-title-book", "{display_title}" }
                        if !primary_author.is_empty() {
                            span { class: "me-page-title-author", "{primary_author}" }
                        }
                    }
                    div { class: "mono me-page-hint",
                        "changes apply on save"
                    }
                }
            }

            // ── Two-column layout ──────────────────────────────────
            div { class: "me-layout",

                // Form column
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

                // ── Sticky sidebar ─────────────────────────────────
                aside { class: "me-sidebar",

                    // Cover preview
                    div { class: "card me-sidebar-card",
                        div { class: "me-sidebar-head",
                            div { class: "label", "Cover" }
                        }
                        div { class: "me-cover-preview",
                            Cover { book: book.clone() }
                        }
                        // Cover upload deferred to v2 (cover picker gallery)
                        div { class: "mono me-cover-hint",
                            if book.cover_url.is_some() {
                                "extracted from file"
                            } else {
                                "no cover available"
                            }
                        }
                    }

                    // Identifiers (read-only for v1)
                    if !book.identifiers.is_empty() {
                        div { class: "card me-sidebar-card",
                            div { class: "label", style: "margin-bottom: 12px;", "Identifiers" }
                            div { class: "me-ident-list",
                                for ident in book.identifiers.iter() {
                                    div {
                                        key: "{ident.scheme:?}-{ident.value}",
                                        class: "me-ident-row",
                                        span { class: "label me-ident-key",
                                            {ident.scheme.clone().unwrap_or_else(|| "ID".into())}
                                        }
                                        span { class: "mono me-ident-val",
                                            {ident.value.clone()}
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Override status
                    if book.has_override {
                        div { class: "card me-sidebar-card",
                            div { class: "label", style: "margin-bottom: 8px;", "Override active" }
                            p { class: "mono", style: "font-size: 11px; color: var(--ink-2);",
                                "This book has metadata overrides. Saving will update them; discarding will leave existing overrides intact."
                            }
                            button {
                                class: "btn ghost sm",
                                style: "margin-top: 10px; width: 100%; justify-content: center;",
                                "data-testid": "revert-overrides",
                                disabled: saving(),
                                onclick: {
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
                                                    // Navigate back to the detail page
                                                    let nav = navigator();
                                                    nav.push(Route::BookDetail { uuid: uuid.clone() });
                                                }
                                                Err(e) => save_error.set(Some(e.to_string())),
                                            }
                                            saving.set(false);
                                        });
                                    }
                                },
                                "Revert to scanned values"
                            }
                        }
                    }
                }
            }

            // ── Sticky save bar ────────────────────────────────────
            div { class: "me-save-bar",
                if dirty_count() > 0 {
                    span { class: "me-dirty-dot" }
                    span { class: "me-dirty-label",
                        {format!("{} field{} edited", dirty_count(), if dirty_count() != 1 { "s" } else { "" })}
                    }
                    span { class: "mono me-dirty-names",
                        {dirty_fields().join(" \u{b7} ")}
                    }
                } else {
                    span { class: "mono me-dirty-label", style: "color: var(--ink-3);",
                        "No changes"
                    }
                }

                if let Some(err) = save_error() {
                    span { class: "mono", style: "color: var(--bad); font-size: 12px; margin-left: 8px;",
                        "{err}"
                    }
                }

                div { class: "me-save-actions",
                    // Discard — navigates back without saving
                    Link {
                        to: Route::BookDetail { uuid: uuid.clone() },
                        class: "btn ghost",
                        "data-testid": "me-discard",
                        "Discard"
                    }

                    // Save
                    button {
                        class: "btn primary",
                        "data-testid": "me-save",
                        disabled: dirty_count() == 0 || saving(),
                        onclick: {
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
                                        &title(),
                                        &description(),
                                        &publisher(),
                                        &published(),
                                        &language(),
                                        &series(),
                                        &series_index(),
                                        &authors(),
                                        &tags(),
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
                        },
                        {
                            if saving() {
                                "Saving\u{2026}".to_string()
                            } else if dirty_count() > 0 {
                                format!("Save \u{b7} {} field{}", dirty_count(), if dirty_count() != 1 { "s" } else { "" })
                            } else {
                                "Save".to_string()
                            }
                        }
                    }
                }
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

#[allow(clippy::too_many_arguments)]
fn build_overrides(
    orig: &EbookMetadata,
    title: &str,
    description: &str,
    publisher: &str,
    published: &str,
    language: &str,
    series: &str,
    series_index: &str,
    authors: &[String],
    tags: &[String],
) -> MetadataOverrides {
    let opt = |new: &str, old: Option<&str>| -> Option<String> {
        let old_val = old.unwrap_or("");
        if new != old_val {
            Some(new.to_string())
        } else {
            None
        }
    };

    let orig_authors: Vec<String> = orig.creators.iter().map(|c| c.name.clone()).collect();
    let creators = if authors != orig_authors.as_slice() {
        Some(
            authors
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

    let subjects = if tags != orig.subjects.as_slice() {
        Some(tags.to_vec())
    } else {
        None
    };

    MetadataOverrides {
        title: opt(title, orig.title.as_deref()),
        description: opt(description, orig.description.as_deref()),
        publisher: opt(publisher, orig.publisher.as_deref()),
        published: opt(published, orig.published.as_deref()),
        language: opt(language, orig.language.as_deref()),
        series: opt(series, orig.series.as_deref()),
        series_index: opt(series_index, orig.series_index.as_deref()),
        creators,
        subjects,
    }
}

// ---------------------------------------------------------------------------
// Page-local sub-components — markup-only adapters.
// ---------------------------------------------------------------------------

/// Label with optional "EDITED" badge and hint text.
/// Renders a `<label for=…>` so screen readers associate it with the input.
#[component]
fn MeLabel(
    text: String,
    #[props(default)] edited: bool,
    #[props(default)] hint: String,
    /// The `id` of the input this label targets.
    #[props(default)]
    target: String,
) -> Element {
    rsx! {
        label { class: "me-label", r#for: target,
            span { "{text}" }
            if edited {
                span { class: "mono me-label-edited", "\u{b7} EDITED" }
            }
            if !hint.is_empty() {
                span { class: "mono me-label-hint", "{hint}" }
            }
        }
    }
}

/// Derive a stable input `id` from a label string (lowercase, hyphens for spaces).
fn label_to_id(label: &str) -> String {
    format!(
        "me-{}",
        label
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
    )
}

/// Single-line input field in the form grid.
#[component]
fn MeField(
    label: String,
    value: Signal<String>,
    on_change: EventHandler<String>,
    #[props(default)] w: i32,
    #[props(default)] big: bool,
    #[props(default)] serif: bool,
    #[props(default)] mono: bool,
    #[props(default)] edited: bool,
    #[props(default)] locked: bool,
    #[props(default)] hint: String,
    #[props(default)] placeholder: String,
) -> Element {
    let col_class = match w {
        2 => "me-field me-field-w2",
        _ => "me-field",
    };
    let input_class = if big && serif {
        "me-input me-input-big me-input-serif"
    } else if mono {
        "me-input me-input-mono"
    } else if serif {
        "me-input me-input-serif"
    } else {
        "me-input"
    };
    let border_class = if edited { " me-input-edited" } else { "" };

    let field_id = label_to_id(&label);

    rsx! {
        div { class: col_class,
            MeLabel { text: label.clone(), edited, hint, target: field_id.clone() }
            input {
                id: field_id,
                class: "{input_class}{border_class}",
                value: "{value}",
                placeholder: if placeholder.is_empty() { label } else { placeholder },
                readonly: locked,
                disabled: locked,
                oninput: move |e| on_change.call(e.value()),
            }
        }
    }
}

/// Multi-line textarea field.
#[component]
fn MeArea(
    label: String,
    value: Signal<String>,
    on_change: EventHandler<String>,
    #[props(default = 4)] rows: i32,
    #[props(default)] edited: bool,
    #[props(default)] hint: String,
) -> Element {
    let border_class = if edited { " me-input-edited" } else { "" };
    let field_id = label_to_id(&label);

    rsx! {
        div { class: "me-field me-field-full",
            MeLabel { text: label.clone(), edited, hint, target: field_id.clone() }
            textarea {
                id: field_id,
                class: "me-textarea{border_class}",
                rows: "{rows}",
                value: "{value}",
                oninput: move |e| on_change.call(e.value()),
            }
        }
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

    #[test]
    fn label_to_id_slugifies_spaces_and_case() {
        assert_eq!(label_to_id("Title"), "me-title");
        assert_eq!(label_to_id("Book #"), "me-book--");
        assert_eq!(label_to_id("Published Date"), "me-published-date");
    }

    #[test]
    fn label_to_id_replaces_all_non_alphanumeric() {
        assert_eq!(label_to_id("A/B:C"), "me-a-b-c");
        assert_eq!(label_to_id(""), "me-");
    }

    #[test]
    fn build_overrides_no_changes_yields_all_none() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
        let ov = build_overrides(
            &orig,
            "Dune",
            "",
            "",
            "",
            "",
            "",
            "",
            &["Frank Herbert".to_string()],
            &["scifi".to_string()],
        );
        assert_eq!(ov, MetadataOverrides::default());
    }

    #[test]
    fn build_overrides_sets_only_changed_scalar_fields() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &[]);
        let ov = build_overrides(
            &orig,
            "Dune: Messiah",
            "",
            "Ace",
            "",
            "",
            "",
            "",
            &["Frank Herbert".to_string()],
            &[],
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
        let ov = build_overrides(&orig, "", "", "", "", "", "", "", &[], &[]);
        assert_eq!(ov.title.as_deref(), Some(""));
    }

    #[test]
    fn build_overrides_replaces_full_creator_and_subject_lists() {
        let orig = book_with(Some("Dune"), &["Frank Herbert"], &["scifi"]);
        let ov = build_overrides(
            &orig,
            "Dune",
            "",
            "",
            "",
            "",
            "",
            "",
            &["Frank Herbert".to_string(), "Brian Herbert".to_string()],
            &["scifi".to_string(), "classic".to_string()],
        );
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
            "Dune",
            "",
            "",
            "",
            "",
            "",
            "",
            &["Frank Herbert".to_string()],
            &["scifi".to_string()],
        );
        assert!(ov.creators.is_none());
        assert!(ov.subjects.is_none());
    }
}
