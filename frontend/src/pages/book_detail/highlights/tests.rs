use super::locator::highlight_locator;
use super::*;

#[test]
fn highlight_locator_names_the_spine_section_for_a_range_cfi() {
    // epub.js emits ranges as `epubcfi(<common parent>,<start>,<end>)`; the
    // spine step is the `/14` before the `!`.
    assert_eq!(
        highlight_locator("epubcfi(/6/14[chap03]!/4/2,/1:0,/1:120)"),
        Some("Section 7".to_string())
    );
}

#[test]
fn highlight_locator_reads_a_point_cfi_without_an_id_assertion() {
    assert_eq!(
        highlight_locator("epubcfi(/6/4!/4/2/2/1:15)"),
        Some("Section 2".to_string())
    );
}

#[test]
fn highlight_locator_tolerates_surrounding_whitespace() {
    assert_eq!(
        highlight_locator("  epubcfi(/6/8!/4)  "),
        Some("Section 4".to_string())
    );
}

#[test]
fn highlight_locator_returns_none_for_unparseable_input() {
    // Not a CFI at all, and a CFI whose wrapper never closes.
    assert_eq!(highlight_locator(""), None);
    assert_eq!(highlight_locator("chapter 4, paragraph 2"), None);
    assert_eq!(highlight_locator("epubcfi(/6/14!/4/2"), None);
}

#[test]
fn highlight_locator_returns_none_when_the_spine_step_is_not_an_element() {
    // Odd steps address text nodes and `/0` isn't a step at all — halving
    // either would invent a section number.
    assert_eq!(highlight_locator("epubcfi(/6/7!/4/2)"), None);
    assert_eq!(highlight_locator("epubcfi(/6/0!/4/2)"), None);
    assert_eq!(highlight_locator("epubcfi(/6/x!/4/2)"), None);
}

#[test]
fn passages_kicker_singularizes_and_reports_the_empty_case() {
    assert_eq!(passages_kicker(0), "Quotes \u{00b7} none saved");
    assert_eq!(passages_kicker(1), "Quotes \u{00b7} 1 saved passage");
    assert_eq!(passages_kicker(4), "Quotes \u{00b7} 4 saved passages");
}

#[test]
fn percent_encode_escapes_every_cfi_delimiter() {
    // `/`, `[`, `]`, `,`, `:` and `!` all carry meaning in a query string.
    assert_eq!(
        percent_encode("epubcfi(/6/14[c]!/4,/1:0)"),
        "epubcfi%28%2F6%2F14%5Bc%5D%21%2F4%2C%2F1%3A0%29"
    );
}

#[test]
fn percent_encode_leaves_unreserved_characters_alone() {
    assert_eq!(
        percent_encode("a-Z_0.9~"),
        "a-Z_0.9~",
        "unreserved characters must survive verbatim"
    );
}

#[test]
fn percent_encode_escapes_multibyte_characters_per_utf8_byte() {
    assert_eq!(percent_encode("é"), "%C3%A9");
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(feature = "server")]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;
    use omnibus_shared::HighlightColor;

    fn section_first_paint() -> Element {
        rsx! {
            BdHighlightsSection { uuid: "book-uuid".to_string() }
        }
    }

    #[test]
    fn section_first_paint_shows_the_empty_state_before_the_post_mount_load() {
        // The load effect hasn't resolved after one rebuild, so the list is
        // empty — exactly what the client hydrates against (rule 07).
        let html = render_in_vdom(section_first_paint);
        assert!(html.contains("data-testid=\"highlights-section\""));
        assert!(html.contains("data-testid=\"highlights-empty\""));
        assert!(html.contains("Quotes \u{00b7} none saved"));
        assert!(!html.contains("data-testid=\"highlights-list\""));
    }

    fn seeded_highlight(note: Option<&str>, text: Option<&str>) -> Highlight {
        Highlight {
            id: 7,
            book_uuid: "book-uuid".to_string(),
            epub_cfi_range: "epubcfi(/6/14[chap03]!/4/2,/1:0,/1:12)".to_string(),
            color: HighlightColor::Violet,
            note: note.map(str::to_string),
            text: text.map(str::to_string),
            client_id: None,
            created_at: 1_779_019_200,
        }
    }

    fn card_with_note() -> Element {
        rsx! {
            BdHighlightCard {
                highlight: seeded_highlight(Some("Worth revisiting"), Some("The Beauty of the House")),
                highlights: use_signal(|| vec![seeded_highlight(None, None)]),
                server_url: String::new(),
            }
        }
    }

    #[test]
    fn card_renders_the_quote_note_locator_and_deep_link() {
        let html = render_in_vdom(card_with_note);
        assert!(html.contains("The Beauty of the House"));
        assert!(html.contains("Worth revisiting"));
        assert!(html.contains("Section 7 \u{00b7} saved May 17, 2026"));
        // The reader link carries the percent-encoded CFI so the passage,
        // not the resume point, is where the reader opens.
        assert!(
            html.contains(
                "/read/book-uuid?cfi=epubcfi%28%2F6%2F14%5Bchap03%5D%21%2F4%2F2%2C%2F1%3A0%2C%2F1%3A12%29"
            ),
            "expected a cfi deep link, got: {html}"
        );
        assert!(html.contains("--hl-violet"));
    }

    fn card_without_text() -> Element {
        rsx! {
            BdHighlightCard {
                highlight: seeded_highlight(None, None),
                highlights: use_signal(Vec::new),
                server_url: String::new(),
            }
        }
    }

    #[test]
    fn card_disables_copy_for_a_highlight_saved_before_the_text_column() {
        // Pre-migration-0030 rows carry no prose, so Copy would be a silent
        // no-op; the control is disabled instead.
        let html = render_in_vdom(card_without_text);
        assert!(html.contains("(highlighted passage)"));
        assert!(!html.contains("data-testid=\"highlight-note\""));
        let copy_idx = html
            .find("data-testid=\"highlight-copy\"")
            .expect("copy button rendered");
        let button_end = html[copy_idx..].find('>').expect("button tag closes") + copy_idx;
        assert!(
            html[copy_idx..button_end].contains("disabled"),
            "copy button should be disabled: {}",
            &html[copy_idx..button_end]
        );
    }
}
