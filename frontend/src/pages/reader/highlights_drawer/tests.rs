//! SSR render-smoke coverage for `HighlightRow`'s anchorless (no-CFI)
//! branch: a Kobo-origin highlight has nothing painted in the viewer to jump
//! to, but recolor and delete must still be offered. Needs the `server`
//! feature (`dioxus::ssr`).

#[cfg(feature = "server")]
use super::*;

#[cfg(feature = "server")]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn seeded(cfi: Option<&str>) -> Highlight {
        Highlight {
            id: 7,
            book_uuid: "book-uuid".to_string(),
            epub_cfi_range: cfi.map(str::to_string),
            color: HighlightColor::Violet,
            note: None,
            text: Some("the highlighted passage".to_string()),
            client_id: None,
            created_at: 1_779_019_200,
        }
    }

    fn anchored_row() -> Element {
        rsx! {
            HighlightRow {
                highlight: seeded(Some("epubcfi(/6/14[chap03]!/4/2,/1:0,/1:12)")),
                highlights: use_signal(Vec::new),
                on_quote: move |_| {},
                on_edit_note: move |_| {},
            }
        }
    }

    fn anchorless_row() -> Element {
        rsx! {
            HighlightRow {
                highlight: seeded(None),
                highlights: use_signal(Vec::new),
                on_quote: move |_| {},
                on_edit_note: move |_| {},
            }
        }
    }

    /// Pull the opening tag for the first match of `testid_or_class` so a
    /// `disabled` assertion doesn't accidentally match a sibling element.
    fn opening_tag<'a>(html: &'a str, needle: &str) -> &'a str {
        let start = html.find(needle).expect("element rendered");
        let end = html[start..].find('>').expect("tag closes") + start;
        &html[start..end]
    }

    #[test]
    fn highlight_row_disables_the_quote_jump_for_an_anchorless_kobo_highlight() {
        // No CFI, nowhere to navigate: the quote button still shows the
        // passage but can't jump to it.
        let html = render_in_vdom(anchorless_row);
        assert!(opening_tag(&html, "rd-hl-quote").contains("disabled"));
        assert!(html.contains("the highlighted passage"));
        // Recolor and delete are unaffected — a Kobo row still supports
        // both even with nothing painted in the viewer to repaint/remove.
        assert!(html.contains("data-testid=\"reader-highlight-recolor-amber\""));
        assert!(html.contains("data-testid=\"highlight-delete\""));
    }

    #[test]
    fn highlight_row_enables_the_quote_jump_for_an_anchored_highlight() {
        let html = render_in_vdom(anchored_row);
        assert!(!opening_tag(&html, "rd-hl-quote").contains("disabled"));
    }
}
