//! Body grid of the book-detail page — two-column main (public journal feed + highlights stub + cover-fan rail) plus a sticky right rail (file details, [`FormatSwitcher`], series info, insights).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, Identifier};

use crate::components::atrium::Cover;
use crate::components::FormatSwitcher;
use crate::Route;

use super::journal::BdJournalSection;
use super::{BdInsightCell, BdMetaRow, BdSectionHead};

/// Main column: public journal feed, highlights stub, from-the-same-hand fan, suggestions stub.
#[component]
pub(super) fn BdBodyMain(
    uuid: String,
    title: String,
    primary_author: String,
    author_books: Vec<EbookMetadata>,
) -> Element {
    rsx! {
        div { class: "bd-body-main",
            BdJournalSection { uuid }
            div { class: "divider" }
            BdSectionHead { kicker: "0 highlights".to_string(), title: "Passages you saved".to_string() }
            div { class: "bd-journal-empty card", aria_hidden: "true",
                p { class: "mono", "No highlights saved yet." }
                p { class: "bd-stub-hint", "Highlights land in F3.2." }
            }
            div { class: "divider" }
            BdSectionHead {
                kicker: if primary_author.is_empty() { "More to read".to_string() } else { format!("More by {primary_author}") },
                title: "From the same hand".to_string(),
            }
            if author_books.is_empty() {
                div { class: "bd-author-books-empty card", "data-testid": "from-same-hand-empty",
                    p { class: "mono", "No other books by this author in your library." }
                }
            } else {
                div { class: "bd-author-books-row", "data-testid": "from-same-hand",
                    for ab in author_books.iter() {
                        Link {
                            key: "{ab.id}",
                            to: Route::BookDetail { uuid: ab.unique_identifier.clone().unwrap_or_default() },
                            class: "bd-author-book-tile",
                            "data-testid": "from-same-hand-tile",
                            Cover { book: ab.clone() }
                        }
                    }
                }
            }
            div { class: "divider" }
            BdSectionHead {
                kicker: format!("If you liked {title}\u{2026}"),
                title: "Suggested for you".to_string(),
            }
            div { class: "bd-stub-strip card", aria_hidden: "true",
                p { class: "bd-stub-hint mono", "Suggestions land in F3.3." }
            }
        }
    }
}

/// Sticky rail: file-details card, series/standalone card, reading-insights card.
#[component]
pub(super) fn BdRailSection(
    b: EbookMetadata,
    title: String,
    authors_line: String,
    series: Option<String>,
    merge_button: Option<Element>,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    rsx! {
        aside { class: "bd-rail",
            div { class: "card",
                div { class: "label bd-rail-head", "File details" }
                table { class: "bd-meta-table mono",
                    tbody {
                        BdMetaRow { k: "Title".to_string(), v: title.clone() }
                        if !authors_line.is_empty() {
                            BdMetaRow { k: "Author".to_string(), v: authors_line.clone() }
                        }
                        if let Some(p) = b.publisher.clone() { BdMetaRow { k: "Pub.".to_string(), v: p } }
                        if let Some(d) = b.published.clone() { BdMetaRow { k: "Date".to_string(), v: d } }
                        if let Some(l) = b.language.clone() { BdMetaRow { k: "Language".to_string(), v: l } }
                        for ident in b.identifiers.iter() {
                            BdMetaRow {
                                key: "{bd_identifier_key(ident)}",
                                k: ident.scheme.clone().unwrap_or_else(|| "ID".into()),
                                v: ident.value.clone(),
                            }
                        }
                    }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Formats" }
                FormatSwitcher {
                    formats: b.formats.clone(),
                    uuid: uuid.clone(),
                    book_files: b.book_files.clone(),
                }
                Link {
                    to: Route::MetadataEdit { uuid: uuid.clone() },
                    class: "btn ghost sm bd-rail-edit",
                    "data-testid": "edit-metadata",
                    "Edit metadata\u{2026}"
                }
                {merge_button}
            }
            div { class: "card",
                if let Some(s) = series.as_ref() {
                    div { class: "label bd-rail-head", "Series" }
                    if let Some(sid) = b.series_id {
                        Link { to: Route::SeriesDetail { id: sid }, class: "bd-rail-body bd-series-link", "{s}" }
                    } else {
                        p { class: "bd-rail-body", "{s}" }
                    }
                } else {
                    div { class: "label bd-rail-head", "Standalone" }
                    p { class: "bd-rail-body", "Not part of a series." }
                }
            }
            div { class: "card",
                div { class: "bd-insights-head",
                    div { class: "label", "Insights" }
                    span { class: "mono bd-insights-tag", "this book" }
                }
                div { class: "bd-insights-grid", aria_hidden: "true",
                    BdInsightCell { label: "Started".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Time read".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Sessions".to_string(), value: "—".to_string() }
                    BdInsightCell { label: "Pace".to_string(), value: "—".to_string() }
                }
                div { class: "divider" }
                div { class: "label bd-rail-head", "Activity · last 22 days" }
                div { class: "bd-activity-bar", aria_hidden: "true",
                    for _ in 0..22u32 { i { class: "bd-activity-tick" } }
                }
                div { class: "bd-activity-axis mono",
                    span { "3wk ago" }
                    span { "minutes read · by day" }
                    span { "today" }
                }
            }
        }
    }
}

/// Collision-free list key for an identifier row. A book can carry several
/// identifiers sharing one `scheme` (the projection keeps every distinct
/// value per scheme), so the key folds in `value` to stay unique among the
/// keyed siblings — Dioxus panics when two keyed siblings share a key.
///
/// Both fields are `Debug`-quoted (not joined with a plain separator): a raw
/// `scheme|value` join collides when the data itself contains the delimiter
/// (`scheme="a|b", value="c"` vs `scheme="a", value="b|c"`), which would
/// reintroduce the very panic this guards against. `Debug` escapes embedded
/// quotes/backslashes, so the `(scheme, value)` pair maps injectively to the
/// key.
fn bd_identifier_key(ident: &Identifier) -> String {
    format!("{:?}\u{1f}{:?}", ident.scheme, ident.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bd_identifier_key_is_unique_for_same_scheme_distinct_values() {
        // The book-detail crash repro: two `unknown`-scheme identifiers on
        // one book must not collide on the rendered list key.
        let isbn = Identifier {
            value: "978-1-938570-40-7".into(),
            scheme: Some("unknown".into()),
        };
        let urn = Identifier {
            value: "urn:uuid:c0e51a66-085f-4805-b116-a0d451d281bd".into(),
            scheme: Some("unknown".into()),
        };
        assert_ne!(bd_identifier_key(&isbn), bd_identifier_key(&urn));
    }

    #[test]
    fn bd_identifier_key_is_unique_for_schemeless_distinct_values() {
        let a = Identifier {
            value: "a".into(),
            scheme: None,
        };
        let b = Identifier {
            value: "b".into(),
            scheme: None,
        };
        assert_ne!(bd_identifier_key(&a), bd_identifier_key(&b));
    }

    #[test]
    fn bd_identifier_key_does_not_collide_when_data_contains_the_delimiter() {
        // A naive `scheme|value` join would map both of these to "a|b|c";
        // the `Debug`-quoted encoding keeps them distinct.
        let split_scheme = Identifier {
            value: "c".into(),
            scheme: Some("a|b".into()),
        };
        let split_value = Identifier {
            value: "b|c".into(),
            scheme: Some("a".into()),
        };
        assert_ne!(
            bd_identifier_key(&split_scheme),
            bd_identifier_key(&split_value)
        );
    }
}
