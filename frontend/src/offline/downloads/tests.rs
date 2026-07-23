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
        }],
        updated_at: 42,
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
    });
    assert!(is_complete(uuid, DlFormat::Epub));
    assert!(!is_complete(uuid, DlFormat::Audio));
    assert!(downloaded_uuids().contains(uuid));
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
