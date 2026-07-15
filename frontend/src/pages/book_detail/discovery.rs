//! Pure display helpers for the book-detail discovery blocks (from-the-same-hand
//! author cluster + Hardcover suggestions), shared by the web [`super::body`] and
//! [`super::mobile`] layouts so both render identical titles/years/labels.

use dioxus::prelude::*;
use omnibus_shared::{BookSuggestion, Contributor, EbookMetadata};

/// Display title for a same-hand tile — the book title, falling back to the
/// on-disk filename (mirrors the shared `Cover` plate fallback).
pub(super) fn same_hand_title(b: &EbookMetadata) -> String {
    match b.title.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => b.filename.clone(),
    }
}

/// Author surname for the empty-state note, falling back to a generic label
/// when the author name is unknown so the sentence stays well-formed.
pub(super) fn same_hand_author_label(primary_author: &str) -> String {
    match primary_author.split_whitespace().last() {
        Some(last) => last.to_string(),
        None => "this author".to_string(),
    }
}

/// Four-digit publication year for the tile subline, or `None` when absent.
pub(super) fn same_hand_year(b: &EbookMetadata) -> Option<String> {
    b.published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .filter(|y| !y.is_empty())
        .map(str::to_string)
}

/// Build a minimal [`EbookMetadata`] so the shared `Cover` can render a
/// suggestion's title/author plate when no cover image is available.
pub(super) fn suggestion_cover_book(s: &BookSuggestion) -> EbookMetadata {
    EbookMetadata {
        title: Some(s.title.clone()),
        creators: vec![Contributor {
            name: s.author.clone(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Absolute cover URL (server base + relative path), or `None` when the
/// suggestion has no cached cover (the plate fallback then applies).
pub(super) fn cover_src(server_url: &str, s: &BookSuggestion) -> Option<String> {
    s.cover_url
        .as_deref()
        .map(|path| crate::media_url(server_url, path))
}

/// "on N lists" relevance label (singular-aware).
pub(super) fn list_count_label(n: i64) -> String {
    if n == 1 {
        "on 1 list".to_string()
    } else {
        format!("on {n} lists")
    }
}

/// Inline loading spinner for the Hardcover suggestions pending state.
/// Rendered identically by [`super::body`] and [`super::mobile`] while a
/// lookup is in flight; the adjacent copy already carries the accessible
/// label, so the icon itself is `aria-hidden`.
#[component]
pub(super) fn SuggestionsSpinner() -> Element {
    rsx! {
        span { class: "suggest-spinner", "data-testid": "suggestions-spinner", aria_hidden: "true" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_hand_title_prefers_title_over_filename() {
        let b = EbookMetadata {
            title: Some("Piranesi".into()),
            filename: "piranesi.epub".into(),
            ..Default::default()
        };
        assert_eq!(same_hand_title(&b), "Piranesi");
    }

    #[test]
    fn same_hand_title_falls_back_to_filename_when_title_blank() {
        // `None`, empty, and whitespace-only titles all fall back so a tile
        // never renders a blank caption.
        for title in [None, Some("".into()), Some("   ".into())] {
            let b = EbookMetadata {
                title,
                filename: "dune.epub".into(),
                ..Default::default()
            };
            assert_eq!(same_hand_title(&b), "dune.epub");
        }
    }

    #[test]
    fn same_hand_year_extracts_four_digit_prefix() {
        let b = EbookMetadata {
            published: Some("2020-09-15".into()),
            ..Default::default()
        };
        assert_eq!(same_hand_year(&b), Some("2020".to_string()));
    }

    #[test]
    fn same_hand_year_is_none_when_missing_or_too_short() {
        let missing = EbookMetadata::default();
        assert_eq!(same_hand_year(&missing), None);
        let short = EbookMetadata {
            published: Some("20".into()),
            ..Default::default()
        };
        // `get(0..4)` returns `None` for a string shorter than four bytes.
        assert_eq!(same_hand_year(&short), None);
    }

    #[test]
    fn same_hand_author_label_uses_surname_when_present() {
        assert_eq!(same_hand_author_label("Ada Lovelace"), "Lovelace");
        assert_eq!(same_hand_author_label("Homer"), "Homer");
    }

    #[test]
    fn same_hand_author_label_falls_back_when_author_blank() {
        assert_eq!(same_hand_author_label(""), "this author");
        assert_eq!(same_hand_author_label("   "), "this author");
    }

    #[test]
    fn list_count_label_is_singular_for_one() {
        assert_eq!(list_count_label(1), "on 1 list");
        assert_eq!(list_count_label(0), "on 0 lists");
        assert_eq!(list_count_label(7), "on 7 lists");
    }

    #[test]
    fn suggestion_cover_book_carries_title_and_author() {
        let s = BookSuggestion {
            title: "Piranesi".into(),
            author: "Susanna Clarke".into(),
            ..Default::default()
        };
        let b = suggestion_cover_book(&s);
        assert_eq!(b.title.as_deref(), Some("Piranesi"));
        assert_eq!(
            b.creators.first().map(|c| c.name.as_str()),
            Some("Susanna Clarke")
        );
    }
}
