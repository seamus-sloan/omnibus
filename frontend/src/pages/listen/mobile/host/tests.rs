use super::{
    audio_blocked_offline, first_audio_file_id, playing_out_of_sync, resolved_file_id,
    should_retry_manifest_with_default,
};
use omnibus_shared::BookFileInfo;

#[test]
fn playing_out_of_sync_flags_only_drifted_state() {
    // Agreement: playing == !paused — nothing to reconcile.
    assert!(!playing_out_of_sync(true, false));
    assert!(!playing_out_of_sync(false, true));
    // Drift: the icon and the element disagree and must be flipped.
    assert!(playing_out_of_sync(false, false));
    assert!(playing_out_of_sync(true, true));
}

#[test]
fn audio_blocked_offline_only_while_offline_without_a_download() {
    let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
    crate::offline::sync::note_offline();
    assert!(
        audio_blocked_offline("u-host-guard"),
        "offline with no completed audio download must block playback"
    );
    crate::offline::sync::note_online();
    assert!(
        !audio_blocked_offline("u-host-guard"),
        "online playback is never blocked"
    );
}

fn bf(id: i64, format: &str, ordinal: i64) -> BookFileInfo {
    BookFileInfo {
        id,
        format: format.into(),
        filename: String::new(),
        ordinal,
        label: None,
        size_bytes: 0,
        path: None,
        etag: None,
    }
}

#[test]
fn first_audio_file_id_skips_a_leading_ebook_format() {
    // A merged ebook+audiobook lists EPUB first (ordered by format); the
    // audiobook manifest must still receive the M4B id, not the EPUB's.
    let files = vec![bf(698, "EPUB", 0), bf(917, "M4B", 0), bf(922, "M4B", 1)];
    assert_eq!(first_audio_file_id(&files), Some(917));
}

#[test]
fn first_audio_file_id_picks_lowest_ordinal() {
    let files = vec![bf(5, "M4B", 2), bf(3, "M4B", 0), bf(4, "M4B", 1)];
    assert_eq!(first_audio_file_id(&files), Some(3));
}

#[test]
fn first_audio_file_id_is_none_without_an_audio_file() {
    let files = vec![bf(1, "EPUB", 0), bf(2, "PDF", 0)];
    assert_eq!(first_audio_file_id(&files), None);
}

#[test]
fn resolved_file_id_names_a_positive_loaded_file() {
    assert_eq!(resolved_file_id(917), Some(917));
}

#[test]
fn resolved_file_id_is_none_for_a_legacy_server_without_identity() {
    // `file_identity()` decodes a pre-#1888 payload as (0, 0); posting a
    // fabricated id would be worse than posting none at all.
    assert_eq!(resolved_file_id(0), None);
}

#[test]
fn should_retry_manifest_with_default_only_on_an_unattributed_fallback() {
    // No explicit selection, and the row's (stale) file differs from
    // the client-computed default — worth one retry.
    assert!(should_retry_manifest_with_default(
        None,
        Some(919),
        Some(917)
    ));
}

#[test]
fn should_retry_manifest_with_default_never_overrides_an_explicit_pick() {
    // A picker selection failing must fail loudly, not silently swap
    // in a different file the listener didn't ask for.
    assert!(!should_retry_manifest_with_default(
        Some(919),
        Some(919),
        Some(917)
    ));
}

#[test]
fn should_retry_manifest_with_default_skips_when_already_the_default() {
    // The failed request already *was* the default — retrying it would
    // just repeat the same failing call.
    assert!(!should_retry_manifest_with_default(
        None,
        Some(917),
        Some(917)
    ));
    assert!(!should_retry_manifest_with_default(None, None, None));
}

use super::needs_reload;

#[test]
fn needs_reload_when_nothing_loaded_or_book_changed() {
    assert!(needs_reload(&None, "book-a", None));
    let loaded = Some(("book-a".to_string(), Some(917)));
    assert!(needs_reload(&loaded, "book-b", Some(940)));
}

#[test]
fn needs_reload_on_explicit_different_part_of_same_book() {
    let loaded = Some(("book-a".to_string(), Some(917)));
    assert!(needs_reload(&loaded, "book-a", Some(918)));
}

#[test]
fn no_reload_for_same_part_or_bare_relink_of_same_book() {
    let loaded = Some(("book-a".to_string(), Some(917)));
    // Same part re-selected → no restart.
    assert!(!needs_reload(&loaded, "book-a", Some(917)));
    // Mini-player relinks with no file_id → keep the current part.
    assert!(!needs_reload(&loaded, "book-a", None));
}
