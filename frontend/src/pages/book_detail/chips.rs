//! Genre + tag chip lists for the book-detail hero, each with an admin-only
//! "+ genres" / "+ tags" pill that swaps the list for the shared
//! [`ChipEditor`] — suggestions come from the library-wide genre/tag clouds,
//! and every add/remove saves a `genres`/`subjects` override immediately (the
//! landing table's inline chip-cell contract).

use dioxus::prelude::*;
use omnibus_shared::MetadataOverrides;

use crate::components::chip_editor::{
    collect_suggestions, ChipEditor, ChipEditorOptions, SuggestionItem,
};
use crate::{data, use_is_admin, use_server_url};

/// Which override-backed chip list a [`BdChipListEditor`] instance edits.
/// Carries the per-kind labels/testids and dispatches the pool fetch and the
/// override save — the same tags/genres split as the landing table's
/// `ChipCellDisplay::tags` / `::genres`.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum BdChipKind {
    Genres,
    Tags,
}

impl BdChipKind {
    fn list_class(self) -> &'static str {
        match self {
            Self::Genres => "bd-tag-list bd-genre-list",
            Self::Tags => "bd-tag-list",
        }
    }

    fn list_testid(self) -> &'static str {
        match self {
            Self::Genres => "bd-genre-list",
            Self::Tags => "bd-tag-list",
        }
    }

    fn chip_class(self) -> &'static str {
        match self {
            Self::Genres => "chip chip-genre",
            Self::Tags => "chip",
        }
    }

    fn add_label(self) -> &'static str {
        match self {
            Self::Genres => "+ genres",
            Self::Tags => "+ tags",
        }
    }

    fn add_testid(self) -> &'static str {
        match self {
            Self::Genres => "bd-add-genres",
            Self::Tags => "bd-add-tags",
        }
    }

    fn editor_testid(self) -> &'static str {
        match self {
            Self::Genres => "bd-genres-editor",
            Self::Tags => "bd-tags-editor",
        }
    }

    fn editor_options(self) -> ChipEditorOptions {
        match self {
            Self::Genres => ChipEditorOptions {
                placeholder: "+ add genre\u{2026}".to_string(),
                aria_remove_prefix: "Remove genre".to_string(),
                testid_prefix: "bd-genres".to_string(),
                autofocus: true,
                dropdown_header: "ADD GENRE".to_string(),
                ..ChipEditorOptions::default()
            },
            Self::Tags => ChipEditorOptions {
                placeholder: "+ add tag\u{2026}".to_string(),
                aria_remove_prefix: "Remove tag".to_string(),
                testid_prefix: "bd-tags".to_string(),
                autofocus: true,
                dropdown_header: "ADD TAG".to_string(),
                ..ChipEditorOptions::default()
            },
        }
    }

    /// Fetch this kind's library-wide suggestion pool.
    async fn fetch_pool(self, url: &str) -> Vec<SuggestionItem> {
        let items: Vec<SuggestionItem> = match self {
            Self::Genres => data::get_genre_cloud(url)
                .await
                .map(|list| {
                    list.into_iter()
                        .map(|g| SuggestionItem::new(g.name, g.count))
                        .collect()
                })
                .unwrap_or_default(),
            Self::Tags => data::get_tag_cloud(url)
                .await
                .map(|list| {
                    list.into_iter()
                        .map(|t| SuggestionItem::new(t.name, t.count))
                        .collect()
                })
                .unwrap_or_default(),
        };
        collect_suggestions(items)
    }

    /// Build the full-replacement override payload for this kind.
    fn overrides(self, values: Vec<String>) -> MetadataOverrides {
        match self {
            Self::Genres => MetadataOverrides {
                genres: Some(values),
                ..Default::default()
            },
            Self::Tags => MetadataOverrides {
                subjects: Some(values),
                ..Default::default()
            },
        }
    }

    /// Pull this kind's canonical list back out of the merged save response.
    fn merged_values(self, merged: omnibus_shared::EbookMetadata) -> Vec<String> {
        match self {
            Self::Genres => merged.genres,
            Self::Tags => merged.subjects,
        }
    }
}

/// Chip list + inline editor for the hero's cover column.
///
/// The "+ genres" / "+ tags" pill is client-only by construction:
/// `use_is_admin` is `false` on SSR and the first WASM paint, so the pill
/// appears after the post-mount `CurrentUser` load (rule 07 holds — same
/// pattern as the hero's wishlist badge).
#[component]
pub(super) fn BdChipListEditor(uuid: String, kind: BdChipKind, values: Vec<String>) -> Element {
    let is_admin = use_is_admin();
    let server_url = use_server_url();
    let mut editing = use_signal(|| false);
    let mut chips: Signal<Vec<String>> = use_signal(|| values.clone());
    // Re-sync the local list when the parent refetches the book (e.g. a
    // merge/undo bumps `refresh`) — but never mid-edit, so a save resolving
    // after edit-close can't be clobbered by a spurious resync (the landing
    // row's guard).
    use_effect(use_reactive!(|values| {
        if !*editing.peek() && *chips.peek() != values {
            chips.set(values);
        }
    }));
    let mut pool: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);

    // Fetch the library-wide suggestion pool lazily, on first open — the
    // pill is admin-only, so eager-fetching on every book view would be
    // waste.
    let fetch_url = server_url.clone();
    let open_editor = move |_| {
        editing.set(true);
        if pool.peek().is_empty() {
            let url = fetch_url.clone();
            spawn(async move {
                pool.set(kind.fetch_pool(&url).await);
            });
        }
    };

    let save_uuid = uuid.clone();
    let on_change = move |new_values: Vec<String>| {
        let uuid = save_uuid.clone();
        let url = server_url.clone();
        spawn(async move {
            // `ChipEditor` has already applied the change optimistically, so
            // every non-success path must resync from the server — otherwise
            // a rejected save leaves a phantom chip until a manual reload.
            let overrides = kind.overrides(new_values);
            let merged = if overrides.validate().is_ok() {
                data::save_overrides(&url, &uuid, &overrides)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            match merged {
                Some(merged) => chips.set(kind.merged_values(merged)),
                None => {
                    if let Ok(Some(book)) = data::get_ebook(&url, &uuid).await {
                        chips.set(kind.merged_values(book));
                    }
                }
            }
        });
    };

    rsx! {
        if editing() {
            div {
                class: "{kind.list_class()} bd-chips-editor-host",
                "data-testid": "{kind.editor_testid()}",
                ChipEditor {
                    values: chips,
                    on_change,
                    suggestions: ReadSignal::from(pool),
                    on_close: move |_| editing.set(false),
                    options: kind.editor_options(),
                }
            }
        } else if !chips().is_empty() || is_admin() {
            ul { class: "{kind.list_class()}", "data-testid": "{kind.list_testid()}",
                for chip in chips().iter() {
                    li { key: "{chip}", class: "{kind.chip_class()}", "{chip}" }
                }
                if is_admin() {
                    li { class: "bd-add-chip-item",
                        button {
                            r#type: "button",
                            class: "chip bd-add-chip",
                            "data-testid": "{kind.add_testid()}",
                            onclick: open_editor,
                            "{kind.add_label()}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn render(kind: BdChipKind, values: Vec<String>) -> String {
        dioxus::ssr::render_element(rsx! {
            BdChipListEditor { uuid: "book-uuid".to_string(), kind, values }
        })
    }

    #[test]
    fn chip_list_editor_renders_tag_chip_per_subject() {
        let html = render(
            BdChipKind::Tags,
            vec!["fiction".into(), "space opera".into()],
        );
        assert!(html.contains("data-testid=\"bd-tag-list\""), "{html}");
        assert!(html.contains("fiction"), "{html}");
        assert!(html.contains("space opera"), "{html}");
    }

    #[test]
    fn chip_list_editor_renders_genre_chips_with_genre_class() {
        let html = render(BdChipKind::Genres, vec!["Horror".into()]);
        assert!(html.contains("data-testid=\"bd-genre-list\""), "{html}");
        assert!(html.contains("chip chip-genre"), "{html}");
        assert!(html.contains("Horror"), "{html}");
    }

    #[test]
    fn chip_list_editor_hides_add_pill_and_list_on_ssr_when_empty() {
        // SSR resolves no `CurrentUser`, so `is_admin` is false and an empty
        // list renders nothing — first-WASM-paint parity (rule 07).
        let html = render(BdChipKind::Tags, Vec::new());
        assert!(!html.contains("data-testid=\"bd-tag-list\""), "{html}");
        assert!(!html.contains("data-testid=\"bd-add-tags\""), "{html}");
    }
}
