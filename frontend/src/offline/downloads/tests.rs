//! Unit tests for the download registry, plan builders, and AC4 isolation.

use omnibus_shared::{BookFileInfo, EbookMetadata, ManifestPart};

use super::*;

fn bf(id: i64, format: &str, ordinal: i64, size_bytes: i64) -> BookFileInfo {
    BookFileInfo {
        id,
        format: format.into(),
        filename: String::new(),
        ordinal,
        label: None,
        size_bytes,
        path: None,
        etag: None,
    }
}

fn book_with_files(files: Vec<BookFileInfo>) -> EbookMetadata {
    EbookMetadata {
        book_files: files,
        ..Default::default()
    }
}

#[test]
fn dl_format_round_trips_through_as_str() {
    assert_eq!(
        DlFormat::parse(DlFormat::Epub.as_str()),
        Some(DlFormat::Epub)
    );
    assert_eq!(
        DlFormat::parse(DlFormat::Audio.as_str()),
        Some(DlFormat::Audio)
    );
    assert_eq!(DlFormat::parse("pdf"), None);
}

#[test]
fn plan_epub_targets_the_download_endpoint_with_optional_file_id() {
    let plan = engine::plan_epub("u1", None);
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].rel, "book.epub");
    assert_eq!(plan[0].url_path, "/api/ebooks/u1/download");
    assert_eq!(plan[0].ordinal, None);

    let plan = engine::plan_epub("u1", Some(42));
    assert_eq!(plan[0].url_path, "/api/ebooks/u1/download?file_id=42");
}

#[test]
fn plan_audio_parts_names_files_by_ordinal_and_mime() {
    let parts = vec![
        ManifestPart {
            ordinal: 0,
            url: "/api/audiobooks/u1/parts/0?file_id=9".into(),
            duration_seconds: 10.0,
            mime: "audio/mp4".into(),
        },
        ManifestPart {
            ordinal: 1,
            url: "/api/audiobooks/u1/parts/1?file_id=9".into(),
            duration_seconds: 12.0,
            mime: "audio/mpeg".into(),
        },
    ];
    let plan = engine::plan_audio_parts(&parts);
    assert_eq!(plan[0].rel, "part-0.m4b");
    assert_eq!(plan[0].ordinal, Some(0));
    assert_eq!(plan[1].rel, "part-1.mp3");
    assert_eq!(plan[1].url_path, "/api/audiobooks/u1/parts/1?file_id=9");
}

#[test]
fn mime_ext_round_trips_with_the_loopback_servers_ext_mime() {
    for (mime, canonical) in [
        ("audio/mp4", "audio/mp4"),
        ("audio/mpeg", "audio/mpeg"),
        ("audio/aac", "audio/aac"),
    ] {
        let ext = engine::mime_ext(mime);
        let served = crate::offline::media::ext_mime(&format!("part-0.{ext}"));
        assert_eq!(
            served, canonical,
            "mime {mime} → ext {ext} → served {served}"
        );
    }
}

#[test]
fn size_estimates_pick_the_matching_formats() {
    let book = book_with_files(vec![
        bf(1, "EPUB", 0, 1_000),
        bf(2, "M4B", 0, 50_000),
        bf(3, "M4B", 1, 60_000),
    ]);
    assert_eq!(engine::epub_size_estimate(&book), Some(1_000));
    assert_eq!(engine::audio_size_estimate(&book), Some(110_000));

    let ebook_only = book_with_files(vec![bf(1, "EPUB", 0, 0)]);
    assert_eq!(engine::epub_size_estimate(&ebook_only), None);
    assert_eq!(engine::audio_size_estimate(&ebook_only), None);
}

#[test]
fn default_audio_file_id_skips_ebook_rows_and_picks_lowest_ordinal() {
    let files = vec![
        bf(698, "EPUB", 0, 0),
        bf(917, "M4B", 1, 0),
        bf(913, "M4B", 0, 0),
    ];
    assert_eq!(default_audio_file_id(&files), Some(913));
    assert_eq!(default_audio_file_id(&[bf(1, "EPUB", 0, 0)]), None);
}

#[test]
fn entry_row_round_trip_preserves_status_and_files() {
    let entry = DownloadEntry {
        book_uuid: "u-round".into(),
        format: DlFormat::Audio,
        title: "A Sea of Glass".into(),
        file_id: Some(9),
        status: DownloadStatus::Complete { bytes: 123 },
        files: vec![PlannedFile {
            rel: "part-0.m4b".into(),
            url_path: "/api/audiobooks/u-round/parts/0?file_id=9".into(),
            ordinal: Some(0),
            bytes: Some(123),
            done: true,
            http_etag: Some("\"resp-1\"".into()),
            source_etag: Some("\"wire-1\"".into()),
        }],
        updated_at: 42,
        stale: false,
    };
    let row = row_from_entry(&entry);
    assert_eq!(row.status, "complete");
    let back = entry_from_row(row).expect("entry");
    assert_eq!(back.status, DownloadStatus::Complete { bytes: 123 });
    assert_eq!(back.files, entry.files);
    assert_eq!(back.file_id, Some(9));

    let mut erroring = entry;
    erroring.status = DownloadStatus::Error {
        message: "boom".into(),
    };
    let back = entry_from_row(row_from_entry(&erroring)).expect("entry");
    assert_eq!(
        back.status,
        DownloadStatus::Error {
            message: "boom".into()
        }
    );
}

#[test]
fn registry_status_and_downloaded_uuids_reflect_upserts() {
    let uuid = "u-registry-test";
    assert_eq!(status(uuid, DlFormat::Epub), DownloadStatus::NotDownloaded);
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Complete { bytes: 10 },
        files: vec![],
        updated_at: 1,
        stale: false,
    });
    assert!(is_complete(uuid, DlFormat::Epub));
    assert!(!is_complete(uuid, DlFormat::Audio));
    assert!(downloaded_uuids().contains(uuid));
}

#[test]
fn update_progress_bytes_mutates_status_in_place_and_preserves_files() {
    let uuid = "u-progress-update";
    let files = vec![PlannedFile {
        rel: "part-0.m4b".into(),
        url_path: "/x".into(),
        ordinal: Some(0),
        bytes: None,
        done: false,
        http_etag: None,
        source_etag: None,
    }];
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Audio,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Downloading {
            downloaded: 0,
            total: Some(100),
        },
        files: files.clone(),
        updated_at: 1,
        stale: false,
    });

    update_progress_bytes(uuid, DlFormat::Audio, 42, Some(100));

    assert_eq!(
        status(uuid, DlFormat::Audio),
        DownloadStatus::Downloading {
            downloaded: 42,
            total: Some(100)
        }
    );
    let entry = get_entry(uuid, DlFormat::Audio).expect("entry");
    assert_eq!(
        entry.files, files,
        "file list must be untouched by a progress bump"
    );
}

#[test]
fn update_progress_bytes_is_a_noop_when_the_entry_is_not_registered() {
    let uuid = "u-progress-missing";
    update_progress_bytes(uuid, DlFormat::Epub, 10, None);
    assert_eq!(status(uuid, DlFormat::Epub), DownloadStatus::NotDownloaded);
}

#[test]
fn update_progress_bytes_clamps_negative_downloaded_to_zero() {
    let uuid = "u-progress-clamp";
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Downloading {
            downloaded: 5,
            total: None,
        },
        files: vec![],
        updated_at: 1,
        stale: false,
    });

    update_progress_bytes(uuid, DlFormat::Epub, -3, None);

    assert_eq!(
        status(uuid, DlFormat::Epub),
        DownloadStatus::Downloading {
            downloaded: 0,
            total: None
        }
    );
}

#[tokio::test]
async fn remove_format_files_deletes_only_the_named_formats_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let book = dir.path().join("u-mixed");
    std::fs::create_dir_all(&book).expect("dir");
    std::fs::write(book.join("book.epub"), b"epub").expect("write");
    std::fs::write(book.join("part-0.m4b"), b"audio0").expect("write");
    std::fs::write(book.join("part-1.m4b"), b"audio1").expect("write");

    remove_format_files(&book, DlFormat::Audio).await;
    assert!(book.join("book.epub").is_file());
    assert!(!book.join("part-0.m4b").exists());
    assert!(!book.join("part-1.m4b").exists());
}

// AC4: removing one book's download directory leaves a sibling download
// byte-identical.
#[tokio::test]
async fn removing_one_books_dir_leaves_siblings_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("uuid-a");
    let b = dir.path().join("uuid-b");
    std::fs::create_dir_all(&a).expect("dir a");
    std::fs::create_dir_all(&b).expect("dir b");
    std::fs::write(a.join("book.epub"), b"aaa").expect("a");
    std::fs::write(b.join("book.epub"), b"bbb-untouched").expect("b");

    tokio::fs::remove_dir_all(&a).await.expect("remove a");
    assert!(!a.exists());
    assert_eq!(
        std::fs::read(b.join("book.epub")).expect("read b"),
        b"bbb-untouched"
    );
}

#[tokio::test]
async fn start_sets_an_error_row_instantly_when_offline() {
    store::init_global_for_tests();
    let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
    crate::offline::sync::note_offline();

    let uuid = "u-offline-start";
    start(
        "http://127.0.0.1:1".into(),
        uuid.into(),
        DlFormat::Epub,
        None,
        "T".into(),
    );
    // No engine spawn, no network attempt — the error row is visible
    // synchronously.
    assert_eq!(
        status(uuid, DlFormat::Epub),
        DownloadStatus::Error {
            message: "You're offline — connect to download".into()
        }
    );
    crate::offline::sync::note_online();
}

#[test]
fn format_bytes_scales_units() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(2_048), "2.0 KB");
    assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    assert_eq!(format_bytes(-5), "0 B");
}

// ── Staleness detection (#1231 AC3) ────────────────────────────────────

fn bf_with_etag(id: i64, format: &str, ordinal: i64, etag: Option<&str>) -> BookFileInfo {
    BookFileInfo {
        etag: etag.map(str::to_string),
        ..bf(id, format, ordinal, 1_000)
    }
}

/// Register a completed download of `uuid` whose snapshot is `source_etag`.
fn seed_complete_download(uuid: &str, format: DlFormat, source_etag: Option<&str>) {
    upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format,
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
            source_etag: source_etag.map(str::to_string),
        }],
        updated_at: 1,
        stale: false,
    });
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

// ── Re-download is non-destructive (#1231) ─────────────────────────────

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
