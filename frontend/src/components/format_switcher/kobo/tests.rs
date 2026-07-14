#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_outcome_reports_success_for_device_write() {
    let (is_error, msg) = super::kobo_outcome("ok", None).unwrap();
    assert!(!is_error);
    assert!(msg.contains("Sent to your Kobo"));
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_outcome_reports_download_fallback_for_unsupported_browser() {
    let (is_error, msg) = super::kobo_outcome("downloaded", None).unwrap();
    assert!(!is_error);
    assert!(msg.contains("downloaded instead"));
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_outcome_stays_silent_when_picker_cancelled() {
    assert!(super::kobo_outcome("cancelled", None).is_none());
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_outcome_surfaces_error_message_when_present() {
    let (is_error, msg) = super::kobo_outcome("error", Some("disk full".into())).unwrap();
    assert!(is_error);
    assert_eq!(msg, "Send to Kobo failed: disk full");
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_outcome_falls_back_to_unknown_error_without_message() {
    let (is_error, msg) = super::kobo_outcome("error", None).unwrap();
    assert!(is_error);
    assert_eq!(msg, "Send to Kobo failed: unknown error");
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_replaces_illegal_chars_and_collapses_space() {
    assert_eq!(
        super::kobo_path_segment("AC/DC: Back \"in\"  Black?").as_deref(),
        Some("AC DC Back in Black"),
    );
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_strips_leading_trailing_dots_and_spaces() {
    assert_eq!(
        super::kobo_path_segment("  .Hidden Title. ").as_deref(),
        Some("Hidden Title"),
    );
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_returns_none_when_nothing_usable_remains() {
    assert_eq!(super::kobo_path_segment("   ///  ").as_deref(), None);
    assert_eq!(super::kobo_path_segment("").as_deref(), None);
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_caps_length_on_a_char_boundary() {
    let seg = super::kobo_path_segment(&"é".repeat(200)).unwrap();
    assert_eq!(seg.chars().count(), 120);
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_subdir_joins_author_and_title() {
    assert_eq!(
        super::kobo_subdir("Ada Lovelace", "Notes on the Engine").as_deref(),
        Some("Ada Lovelace/Notes on the Engine"),
    );
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_subdir_uses_only_the_surviving_segment() {
    assert_eq!(
        super::kobo_subdir("", "Just a Title").as_deref(),
        Some("Just a Title"),
    );
    assert_eq!(
        super::kobo_subdir("Author Only", "   ").as_deref(),
        Some("Author Only"),
    );
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_subdir_is_none_when_both_segments_are_empty() {
    assert_eq!(super::kobo_subdir("  ", "").as_deref(), None);
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_defuses_windows_reserved_names() {
    // Case-insensitive, and ignoring an extension after a dot.
    assert_eq!(super::kobo_path_segment("CON").as_deref(), Some("_CON"));
    assert_eq!(super::kobo_path_segment("nul").as_deref(), Some("_nul"));
    assert_eq!(super::kobo_path_segment("Aux").as_deref(), Some("_Aux"));
    assert_eq!(super::kobo_path_segment("COM4").as_deref(), Some("_COM4"));
    assert_eq!(
        super::kobo_path_segment("lpt9.txt").as_deref(),
        Some("_lpt9.txt")
    );
}

#[cfg(not(feature = "mobile"))]
#[test]
fn kobo_path_segment_leaves_non_reserved_lookalikes_alone() {
    // COM0/LPT0 aren't reserved; and a reserved word as a substring is fine.
    assert_eq!(super::kobo_path_segment("COM0").as_deref(), Some("COM0"));
    assert_eq!(
        super::kobo_path_segment("Console").as_deref(),
        Some("Console")
    );
    assert_eq!(super::kobo_path_segment("Conan").as_deref(), Some("Conan"));
}
