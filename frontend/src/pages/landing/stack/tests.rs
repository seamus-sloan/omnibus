//! Unit tests for the stack's pure derivations: the per-point display facts
//! the fan and the edge ribbon both read, and the page accent the lead book
//! hands to the whole surface.

use omnibus_shared::{Contributor, EbookMetadata, ProgressFormat, ProgressRecord, ResumePoint};

use super::*;

/// `stack_entries` with an empty cache-bust map — every test here is about
/// the derivation, not about cover URLs.
fn stack_entries_for_test(points: &[ResumePoint], server_url: &str) -> Vec<StackEntry> {
    stack_entries(points, server_url, &std::collections::HashMap::new())
}

fn book(uuid: &str, title: &str, accent: Option<&str>, formats: &[&str]) -> EbookMetadata {
    EbookMetadata {
        unique_identifier: Some(uuid.to_string()),
        title: Some(title.to_string()),
        creators: vec![Contributor {
            name: "Susanna Clarke".to_string(),
            ..Default::default()
        }],
        accent: accent.map(str::to_string),
        formats: formats.iter().map(|f| (*f).to_string()).collect(),
        ..Default::default()
    }
}

fn point(uuid: &str, format: ProgressFormat, pct: Option<i64>) -> ResumePoint {
    ResumePoint {
        record: ProgressRecord {
            book_uuid: uuid.to_string(),
            format,
            epub_cfi: None,
            audio_position_seconds: Some(600.0),
            progress_percent: pct,
            kobo_location: None,
            book_file_id: None,
            updated_at: 0,
            client_updated_at: 0,
        },
        book: book(uuid, "Piranesi", Some("oklch(0.7 0.1 200)"), &["epub"]),
        linked: false,
        cross_format: None,
        total_duration_seconds: Some(3600.0),
        chapter_number: None,
        chapter_count: None,
        playback_rate: None,
    }
}

#[test]
fn stack_entries_prefix_the_position_line_with_the_format_being_resumed() {
    let epub = &stack_entries_for_test(&[point("a", ProgressFormat::Epub, Some(55))], "")[0];
    assert!(
        epub.where_line.starts_with("Ebook \u{00b7} "),
        "got {}",
        epub.where_line
    );

    let audio = &stack_entries_for_test(&[point("b", ProgressFormat::Audio, None)], "")[0];
    assert!(
        audio.where_line.starts_with("Audiobook \u{00b7} "),
        "got {}",
        audio.where_line
    );
}

#[test]
fn stack_entries_label_the_veil_with_the_formats_verb_and_the_percent() {
    let epub = &stack_entries_for_test(&[point("a", ProgressFormat::Epub, Some(55))], "")[0];
    assert_eq!(epub.veil_label, "resume \u{00b7} 55%");

    // Audio derives its percent from the position/duration pair.
    let audio = &stack_entries_for_test(&[point("b", ProgressFormat::Audio, None)], "")[0];
    assert_eq!(audio.veil_label, "play \u{00b7} 17%");
}

#[test]
fn stack_entries_fall_back_to_a_bare_verb_when_no_percent_is_known() {
    let mut p = point("a", ProgressFormat::Epub, None);
    p.record.progress_percent = None;
    let entry = &stack_entries_for_test(&[p], "")[0];
    assert_eq!(entry.veil_label, "resume");
    assert_eq!(entry.pct, None);
}

#[test]
fn stack_entries_mark_dual_format_books_linked_or_unlinked_but_never_both() {
    let mut unlinked = point("a", ProgressFormat::Epub, Some(10));
    unlinked.book = book("a", "Piranesi", None, &["epub", "m4b"]);
    let entry = &stack_entries_for_test(&[unlinked.clone()], "")[0];
    assert!(entry.dual_unlinked && !entry.dual_linked);

    let mut linked = unlinked;
    linked.linked = true;
    let entry = &stack_entries_for_test(&[linked], "")[0];
    assert!(entry.dual_linked && !entry.dual_unlinked);

    // A single-format book is neither, so it grows no cross-format chips.
    let entry = &stack_entries_for_test(&[point("c", ProgressFormat::Epub, Some(10))], "")[0];
    assert!(!entry.dual_linked && !entry.dual_unlinked);
}

#[test]
fn lead_accent_style_follows_the_front_book_and_is_empty_without_one() {
    let mut plain = point("b", ProgressFormat::Epub, Some(20));
    plain.book = book("b", "Babel", None, &["epub"]);
    let entries = stack_entries_for_test(&[point("a", ProgressFormat::Epub, Some(10)), plain], "");

    assert_eq!(
        lead_accent_style(&entries, 0),
        "--accent: oklch(0.7 0.1 200);"
    );
    // A book with no stored accent leaves the page on the Atrium default
    // rather than inheriting the previous lead's colour.
    assert_eq!(lead_accent_style(&entries, 1), "");
    // Out of range (a refetch that shortened the list) is the same case.
    assert_eq!(lead_accent_style(&entries, 9), "");
    assert_eq!(lead_accent_style(&[], 0), "");
}
