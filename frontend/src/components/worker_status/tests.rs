use super::*;

#[test]
fn kind_label_covers_all_variants_in_both_tenses() {
    for kind in [
        TaskKind::Scan,
        TaskKind::GenerateThumbs,
        TaskKind::ResolveAuthorPhoto,
        TaskKind::RefetchAuthorPhotos,
        TaskKind::BackfillChapters,
        TaskKind::ResolveSuggestions,
    ] {
        assert!(!kind_label(kind, true).is_empty());
        assert!(!kind_label(kind, false).is_empty());
    }
}

#[test]
fn tallies_changes_omits_zero_buckets_and_joins_the_rest() {
    let tallies = ScanTallies {
        found: 340,
        new: 3,
        changed: 0,
        removed: 2,
        moved: 0,
        unchanged: 335,
    };
    assert_eq!(tallies_changes(&tallies), "3 new · 2 removed");
    assert_eq!(
        active_tallies_line(&tallies),
        "Found 340 — 3 new · 2 removed"
    );
}

#[test]
fn tallies_changes_reads_no_changes_when_every_bucket_is_zero() {
    let tallies = ScanTallies {
        found: 12,
        unchanged: 12,
        ..Default::default()
    };
    assert_eq!(tallies_changes(&tallies), "no changes");
    assert_eq!(active_tallies_line(&tallies), "Found 12 — no changes");
}

#[test]
fn ghost_warning_message_names_the_removed_count_and_total() {
    let warning = GhostFilesWarning {
        removed: 15,
        total: 100,
    };
    let message = ghost_warning_message(&warning);
    assert!(
        message.contains("15"),
        "message must name the count: {message}"
    );
    assert!(
        message.contains("100"),
        "message must name the total: {message}"
    );
}

#[test]
fn bake_errors_message_names_the_count_and_every_failed_book_uuid_under_the_cap() {
    let errors = vec!["uuid-a".to_string(), "uuid-b".to_string()];
    let message = bake_errors_message(&errors);
    assert!(
        message.contains("2 books failed to bake:"),
        "message must name the count: {message}"
    );
    assert!(message.contains("uuid-a"), "{message}");
    assert!(message.contains("uuid-b"), "{message}");
    assert!(!message.contains("more"), "{message}");
}

#[test]
fn bake_errors_message_collapses_uuids_past_the_inline_cap() {
    let errors: Vec<String> = (0..BAKE_ERRORS_INLINE_CAP + 3)
        .map(|i| format!("uuid-{i}"))
        .collect();
    let message = bake_errors_message(&errors);
    assert!(
        message.contains("and 3 more"),
        "message must summarize the overflow: {message}"
    );
    assert!(
        !message.contains(&format!("uuid-{}", BAKE_ERRORS_INLINE_CAP)),
        "message must not name a uuid past the cap: {message}"
    );
}

#[test]
fn bake_errors_message_uses_singular_book_noun_for_one_failure() {
    let errors = vec!["uuid-solo".to_string()];
    let message = bake_errors_message(&errors);
    assert!(message.contains("1 book failed"), "{message}");
}
