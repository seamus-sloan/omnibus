//! Unit tests for the download registry, plan builders, staleness
//! tracking and the validator sweep, split by sub-topic into the sibling
//! modules below; the `BookFileInfo` fixtures they share live here.

#![allow(clippy::await_holding_lock)]

mod registry;
mod staleness;
mod sweep;

use omnibus_shared::{BookFileInfo, EbookMetadata};

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
