//! Tests for the W4 journal stop's excerpt helper and its two-line ladder row.

use super::*;

#[test]
fn journal_excerpt_takes_the_first_non_empty_line_and_strips_markup() {
    assert_eq!(
        journal_excerpt("\n\n## **A held** breath\nsecond line"),
        "A held breath"
    );
}

#[test]
fn journal_excerpt_masks_spoiler_spans_so_they_never_reach_the_row() {
    // The row is always visible, so a `||spoiler||` span has to be blocked
    // out rather than merely un-marked.
    assert_eq!(
        journal_excerpt("she ||dies|| in chapter four"),
        "she \u{2588}\u{2588}\u{2588} in chapter four"
    );
}

#[test]
fn journal_excerpt_truncates_past_the_row_cap() {
    let long = "x".repeat(LADDER_EXCERPT_CHARS + 40);
    let out = journal_excerpt(&long);
    assert_eq!(out.chars().count(), LADDER_EXCERPT_CHARS + 1);
    assert!(out.ends_with('\u{2026}'));
}

// SSR render coverage for the ladder row. Needs the `server` feature
// (`dioxus::ssr`), like the sibling highlights render tests.
#[cfg(feature = "server")]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn seeded_entry() -> JournalEntry {
        JournalEntry {
            id: 7,
            book_uuid: "book-uuid".to_string(),
            author_id: 3,
            author_name: "Mira Reyes".to_string(),
            author_has_avatar: false,
            body_md: "The footnotes have footnotes.".to_string(),
            body_html: "<p>The footnotes have footnotes.</p>".to_string(),
            progress: Some(42),
            status: omnibus_shared::JournalStatus::Published,
            client_id: None,
            created_at: 1_779_019_200,
            updated_at: 1_779_019_200,
        }
    }

    #[component]
    fn RowHost() -> Element {
        let dates_ready = super::super::use_local_dates_ready();
        let open_entry = use_signal(|| None::<i64>);
        render_ladder_row(&seeded_entry(), Some(3), dates_ready, open_entry)
    }

    fn render_row() -> Element {
        rsx! {
            RowHost {}
        }
    }

    #[test]
    fn ladder_row_carries_the_byline_the_excerpt_and_the_read_affordance() {
        // Two lines per the design: author (with the owner's "you") plus the
        // date/progress stamp, then the excerpt ending in `read →`.
        let html = render_in_vdom(render_row);
        assert!(
            html.contains("data-testid=\"journal-ladder-row\""),
            "{html}"
        );
        assert!(html.contains("Mira Reyes"), "{html}");
        assert!(html.contains("you"), "{html}");
        assert!(html.contains("May 17, 2026 \u{b7} at 42%"), "{html}");
        assert!(html.contains("The footnotes have footnotes."), "{html}");
        assert!(html.contains("read \u{2192}"), "{html}");
        // The hint the design never asked for stays gone.
        assert!(!html.contains("tap to open the full entry"), "{html}");
    }

    #[component]
    fn DraftRowHost() -> Element {
        let dates_ready = super::super::use_local_dates_ready();
        let open_entry = use_signal(|| None::<i64>);
        let mut entry = seeded_entry();
        entry.status = omnibus_shared::JournalStatus::Draft;
        entry.progress = None;
        render_ladder_row(&entry, None, dates_ready, open_entry)
    }

    fn render_draft_row() -> Element {
        rsx! {
            DraftRowHost {}
        }
    }

    #[test]
    fn ladder_row_marks_a_draft_and_drops_the_progress_stamp_when_absent() {
        let html = render_in_vdom(render_draft_row);
        assert!(
            html.contains("data-testid=\"journal-draft-chip-row\""),
            "{html}"
        );
        assert!(html.contains("May 17, 2026"), "{html}");
        assert!(!html.contains("at 42%"), "{html}");
        // Not the current user's entry — no "you" chip.
        assert!(!html.contains("\u{b7} you"), "{html}");
    }
}
