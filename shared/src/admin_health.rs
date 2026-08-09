//! Wire types for the admin server-health page (`/admin/health`, #952):
//! index status, worker queue depth, FTS index health, and storage
//! utilization. The fifth section (last errors) reuses
//! [`crate::error_ring::CapturedError`] directly rather than a redundant
//! wrapper.

use serde::{Deserialize, Serialize};

use crate::error_ring::CapturedError;
use crate::worker::TaskKind;

/// Library index summary: configured library paths and row counts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexStatus {
    pub ebook_library_path: Option<String>,
    pub audiobook_library_path: Option<String>,
    pub book_count: i64,
    pub author_count: i64,
    pub series_count: i64,
    /// Books with no `book_files` rows — ghosted by a reindex whose scan no
    /// longer found their file on disk (identity preserved, content gone;
    /// see `.claude/rules/06-migrations.md`'s book-identity section).
    pub ghosted_book_count: i64,
}

/// One [`TaskKind`]'s live queue depth and recent completion timings.
/// Timings are milliseconds (wire-friendly; the db-side `Worker::metrics`
/// uses `std::time::Duration`, which doesn't serialize), oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerQueueEntry {
    pub kind: TaskKind,
    pub queue_depth: usize,
    pub recent_completions_ms: Vec<u64>,
}

/// `books_fts` vs `books` row-count parity. A rebuild
/// (`POST /api/fts/rebuild`) always restores `in_sync`; drift here means a
/// post-commit FTS update failed silently on some earlier write.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FtsHealth {
    pub books_rows: i64,
    pub fts_rows: i64,
    pub in_sync: bool,
}

/// Disk usage across the book library and every on-disk cache.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageStats {
    /// Sum of `book_files.size_bytes` — the on-disk footprint of the
    /// indexed library itself (not a cache, so it carries no cap).
    pub library_bytes: i64,
    pub covers_bytes: u64,
    pub thumbs_bytes: u64,
    pub thumbs_cap_bytes: u64,
    pub hls_bytes: u64,
    pub hls_cap_bytes: u64,
}

/// Full report backing `/admin/health`'s five sections, returned in one
/// request by `GET /api/admin/health` (REST) / `rpc_get_admin_health`
/// (web). No polling — a fresh report is fetched once on page load; see
/// issue #955 for a future live-updating variant.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AdminHealthReport {
    pub index: IndexStatus,
    pub worker_queue: Vec<WorkerQueueEntry>,
    pub fts: FtsHealth,
    pub storage: StorageStats,
    pub last_errors: Vec<CapturedError>,
}
