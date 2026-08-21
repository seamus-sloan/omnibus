//! Unit tests for the `indexer` module, split by sub-topic into the sibling
//! modules below; the `StatEntry` / DB-row builders and the on-disk ebook
//! seeder they share live here.

mod backfill;
mod cbz;
mod diff;
mod diff_moves;
mod guards;
mod merged;
mod progress_reporting;
mod reindex;
mod resilience;
mod staleness;

use crate::books::IndexedRow;
use crate::ebook::StatEntry;
use crate::sync::{sync_books, SyncPlan};
use crate::test_support::indexed;

use super::*;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn entry(name: &str, scan_key: &str, mtime: i64, size: i64) -> StatEntry {
    StatEntry {
        filename: name.into(),
        scan_key: scan_key.into(),
        mtime_epoch: mtime,
        size_bytes: size,
        error: None,
    }
}

/// A file-backed DB row. `scan_key` is the diff key; `uuid` mirrors it so
/// the Removed/Backfill buckets (which carry uuids) keep asserting on the
/// same string the callers pass.
fn row(scan_key: &str, mtime: i64, size: i64) -> IndexedRow {
    IndexedRow {
        uuid: scan_key.into(),
        scan_key: scan_key.into(),
        has_file: true,
        mtime_epoch: mtime,
        size_bytes: size,
    }
}

/// A fileless book row: retained book whose file is gone.
fn fileless_row(scan_key: &str) -> IndexedRow {
    IndexedRow {
        uuid: scan_key.into(),
        scan_key: scan_key.into(),
        has_file: false,
        mtime_epoch: 0,
        size_bytes: 0,
    }
}

/// Index one EPUB through the real `sync_books` write path at `library_path`
/// (a real on-disk dir), writing a matching stub file so a later `reindex`
/// re-finds it. `filename` is the library-relative path.
async fn seed_ebook_at(pool: &SqlitePool, library_path: &str, filename: &str, title: &str) {
    let abs = std::path::Path::new(library_path).join(filename);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, b"not a zip").unwrap();
    let (mtime, size) = {
        let meta = std::fs::metadata(&abs).unwrap();
        (
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            meta.len() as i64,
        )
    };
    // Seed the row with the real stat so the reindex classifies it Unchanged
    // (not Changed) on a healthy pass.
    let mut book = indexed(filename, Some(title), &["Author"], &[], None, None);
    book.mtime_epoch = mtime;
    book.size_bytes = size;
    sync_books(
        pool,
        library_path,
        SyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}
