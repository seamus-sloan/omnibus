//! Staleness against the wire validator: the three-valued `is_stale`,
//! the compared file mirroring the server's own row selection (or an
//! explicitly chosen file id), `note_book` marking and clearing the
//! update flag, a redownload keeping the prior copy and selection, restore
//! after a failed replacement, and applying a batch validator answer.

#![allow(clippy::await_holding_lock)]

use omnibus_shared::BookFileInfo;

use super::super::*;
use super::{bf, book_with_files, seed_complete_download};

fn bf_with_etag(id: i64, format: &str, ordinal: i64, etag: Option<&str>) -> BookFileInfo {
    BookFileInfo {
        etag: etag.map(str::to_string),
        ..bf(id, format, ordinal, 1_000)
    }
}

#[test]
fn is_stale_is_true_when_the_library_file_moved_under_the_download() {
    let uuid = "u-stale-moved";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"old\""));
    let book = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    assert!(is_stale(uuid, DlFormat::Epub, &book));
}

#[test]
fn is_stale_is_false_when_the_validator_is_unchanged() {
    let uuid = "u-stale-same";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"same\""));
    let book = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"same\""))]);
    assert!(!is_stale(uuid, DlFormat::Epub, &book));
}

#[test]
fn is_stale_is_false_when_either_side_has_no_validator() {
    // Not knowing is not the same as knowing it is stale. A registry row
    // written before validators existed, or a `book_files` row the scanner
    // has not stat'd, must not prompt a re-download on a guess.
    let no_snapshot = "u-stale-no-snapshot";
    seed_complete_download(no_snapshot, DlFormat::Epub, None);
    let book = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    assert!(!is_stale(no_snapshot, DlFormat::Epub, &book));

    let no_current = "u-stale-no-current";
    seed_complete_download(no_current, DlFormat::Epub, Some("\"old\""));
    let unstatted = book_with_files(vec![bf_with_etag(1, "EPUB", 0, None)]);
    assert!(!is_stale(no_current, DlFormat::Epub, &unstatted));
}

#[test]
fn is_stale_is_false_for_a_download_that_never_completed() {
    let uuid = "u-stale-incomplete";
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Error {
            message: "boom".into(),
        },
        files: vec![PlannedFile {
            rel: "book.epub".into(),
            url_path: "/x".into(),
            ordinal: None,
            bytes: None,
            done: false,
            http_etag: None,
            source_etag: Some("\"old\"".into()),
        }],
        updated_at: 1,
        stale: false,
    });
    let book = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    assert!(!is_stale(uuid, DlFormat::Epub, &book));
}

#[test]
fn is_stale_is_false_for_a_book_that_was_never_downloaded() {
    let book = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    assert!(!is_stale("u-stale-absent", DlFormat::Epub, &book));
}

#[test]
fn target_file_mirrors_the_servers_own_row_selection() {
    let book = book_with_files(vec![
        bf_with_etag(10, "EPUB", 1, Some("\"epub-1\"")),
        bf_with_etag(11, "EPUB", 0, Some("\"epub-0\"")),
        bf_with_etag(20, "M4B", 1, Some("\"m4b-1\"")),
        bf_with_etag(21, "M4B", 0, Some("\"m4b-0\"")),
    ]);
    // No explicit file: lowest ordinal of the format, matching
    // `db::book_file_path`'s `ORDER BY bf.ordinal LIMIT 1`.
    assert_eq!(
        target_file(&book, DlFormat::Epub, None).map(|f| f.id),
        Some(11)
    );
    assert_eq!(
        target_file(&book, DlFormat::Audio, None).map(|f| f.id),
        Some(21)
    );
    // An explicit file_id wins, and is looked up across every format.
    assert_eq!(
        target_file(&book, DlFormat::Epub, Some(10)).map(|f| f.id),
        Some(10)
    );
    assert_eq!(target_file(&book, DlFormat::Epub, Some(999)), None);
}

#[test]
fn is_stale_follows_an_explicitly_chosen_file_id() {
    let uuid = "u-stale-file-id";
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: Some(10),
        status: DownloadStatus::Complete { bytes: 10 },
        files: vec![PlannedFile {
            rel: "book.epub".into(),
            url_path: "/x?file_id=10".into(),
            ordinal: None,
            bytes: Some(10),
            done: true,
            http_etag: None,
            source_etag: Some("\"epub-1\"".into()),
        }],
        updated_at: 1,
        stale: false,
    });
    // Row 11 changed; row 10 — the one this download came from — did not.
    let book = book_with_files(vec![
        bf_with_etag(10, "EPUB", 1, Some("\"epub-1\"")),
        bf_with_etag(11, "EPUB", 0, Some("\"epub-0-changed\"")),
    ]);
    assert!(
        !is_stale(uuid, DlFormat::Epub, &book),
        "a sibling file changing must not mark this download stale"
    );
}

#[test]
fn note_book_marks_and_clears_the_update_available_flag() {
    let uuid = "u-note-book";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"old\""));
    assert!(!is_marked_stale(uuid, DlFormat::Epub));

    let mut moved = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    moved.unique_identifier = Some(uuid.into());
    note_book(&moved);
    assert!(is_marked_stale(uuid, DlFormat::Epub));

    // The reader re-downloaded elsewhere and the snapshot now matches
    // again: the chip must go away rather than stick until a restart.
    let mut caught_up = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"old\""))]);
    caught_up.unique_identifier = Some(uuid.into());
    note_book(&caught_up);
    assert!(!is_marked_stale(uuid, DlFormat::Epub));
}

#[test]
fn note_book_leaves_the_flag_alone_when_metadata_carries_no_file_rows() {
    // The landing/replica projection has no `book_files`, so it can neither
    // confirm nor deny staleness — it must not clear a flag the per-book
    // read established.
    let uuid = "u-note-book-no-files";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"old\""));
    let mut moved = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    moved.unique_identifier = Some(uuid.into());
    note_book(&moved);
    assert!(is_marked_stale(uuid, DlFormat::Epub));

    let mut projection_only = book_with_files(vec![]);
    projection_only.unique_identifier = Some(uuid.into());
    note_book(&projection_only);
    assert!(
        is_marked_stale(uuid, DlFormat::Epub),
        "a projection without file rows must not silently clear the chip"
    );
}

#[test]
fn note_book_is_a_noop_for_a_book_with_no_uuid() {
    let mut anonymous = book_with_files(vec![bf_with_etag(1, "EPUB", 0, Some("\"new\""))]);
    anonymous.unique_identifier = None;
    note_book(&anonymous);
}

#[test]
fn redownload_keeps_the_files_and_the_prior_selection() {
    // The chip's action must not be remove-then-start: `remove` deletes
    // asynchronously, so pairing them races the deletion against the engine
    // writing the replacement — and discards a readable book before knowing
    // the new one arrives.
    let uuid = "u-redownload";
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "Old title".into(),
        file_id: Some(42),
        status: DownloadStatus::Complete { bytes: 10 },
        files: vec![PlannedFile {
            rel: "book.epub".into(),
            url_path: "/x?file_id=42".into(),
            ordinal: None,
            bytes: Some(10),
            done: true,
            http_etag: Some("\"resp\"".into()),
            source_etag: Some("\"old\"".into()),
        }],
        updated_at: 1,
        stale: true,
    });

    // No runtime in tests, so the engine never spawns; what matters is the
    // registry state the spawn would have run against.
    redownload(
        "http://localhost".into(),
        uuid.into(),
        DlFormat::Epub,
        None,
        "New title".into(),
    );

    let entry = get_entry(uuid, DlFormat::Epub).expect("entry");
    assert_eq!(
        entry.file_id,
        Some(42),
        "a caller that doesn't track the selection must inherit it, not retarget the book"
    );
    assert_eq!(
        entry.files.len(),
        1,
        "the planned files survive so their bytes stay on disk as the fallback"
    );
    assert!(
        !entry.files[0].done,
        "every file is refetched rather than carried forward"
    );
    assert_eq!(
        entry.files[0].http_etag.as_deref(),
        Some("\"resp\""),
        "the response validator is kept so a resume can still offer If-Range"
    );
}

#[test]
fn restore_puts_back_the_copy_a_failed_replacement_was_standing_in_for() {
    let uuid = "u-redownload-restore";
    let previous = DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Complete { bytes: 10 },
        files: vec![PlannedFile {
            rel: "book.epub".into(),
            url_path: "/x".into(),
            ordinal: None,
            bytes: Some(10),
            done: true,
            http_etag: None,
            source_etag: Some("\"old\"".into()),
        }],
        updated_at: 1,
        stale: true,
    };
    upsert(previous.clone());
    redownload(
        "http://localhost".into(),
        uuid.into(),
        DlFormat::Epub,
        None,
        "T".into(),
    );
    assert!(!is_complete(uuid, DlFormat::Epub), "mid-replacement");

    restore(previous);

    // The book the reader already had, still complete and still flagged —
    // the chip invites another try rather than the book vanishing.
    assert!(is_complete(uuid, DlFormat::Epub));
    assert!(is_marked_stale(uuid, DlFormat::Epub));
}

#[test]
fn apply_validators_marks_and_clears_from_a_batch_answer() {
    let uuid = "u-batch-validators";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"old\""));

    let answer = |etag: Option<&str>| omnibus_shared::DownloadValidator {
        book_uuid: uuid.into(),
        format: omnibus_shared::DownloadFormat::Epub,
        file_id: None,
        etag: etag.map(str::to_string),
    };

    apply_validators(&[answer(Some("\"new\""))]);
    assert!(is_marked_stale(uuid, DlFormat::Epub));

    apply_validators(&[answer(Some("\"old\""))]);
    assert!(!is_marked_stale(uuid, DlFormat::Epub));
}

#[test]
fn apply_validators_leaves_the_flag_alone_when_the_server_cannot_answer() {
    // A book, format or file the server no longer has comes back with no
    // etag. That is "can't tell", and must not clear a chip a real
    // comparison set.
    let uuid = "u-batch-unanswerable";
    seed_complete_download(uuid, DlFormat::Epub, Some("\"old\""));
    apply_validators(&[omnibus_shared::DownloadValidator {
        book_uuid: uuid.into(),
        format: omnibus_shared::DownloadFormat::Epub,
        file_id: None,
        etag: Some("\"new\"".into()),
    }]);
    assert!(is_marked_stale(uuid, DlFormat::Epub));

    apply_validators(&[omnibus_shared::DownloadValidator {
        book_uuid: uuid.into(),
        format: omnibus_shared::DownloadFormat::Epub,
        file_id: None,
        etag: None,
    }]);
    assert!(is_marked_stale(uuid, DlFormat::Epub));
}

#[test]
fn completed_validator_queries_names_the_file_each_download_came_from() {
    let uuid = "u-queries";
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Audio,
        title: "T".into(),
        file_id: Some(9),
        status: DownloadStatus::Complete { bytes: 10 },
        files: vec![],
        updated_at: 1,
        stale: false,
    });

    let queries = completed_validator_queries();
    let mine = queries
        .iter()
        .find(|q| q.book_uuid == uuid)
        .expect("the completed download is asked about");
    assert_eq!(mine.format, omnibus_shared::DownloadFormat::Audio);
    assert_eq!(
        mine.file_id,
        Some(9),
        "asking about the default row would report a two-edition book stale when it isn't"
    );
}
