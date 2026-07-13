//! Testable core of the periodic library-rescan background task.
//!
//! The `tokio::time` loop that calls [`periodic_scan_tick`] on a schedule
//! lives in `server::main` (`spawn_periodic_scan`), mirroring
//! `spawn_session_pruner`'s shape. This module owns the "read settings,
//! decide, post" step in isolation so it can be exercised directly through
//! the worker test harness instead of waiting on a real timer.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use super::types::{Task, Worker};

/// Recheck cadence used while `scan_interval_hours` is unset (or a settings
/// read fails), so enabling the setting later takes effect without a
/// restart instead of waiting on a timer that was never armed.
pub const PERIODIC_SCAN_RECHECK: Duration = Duration::from_secs(5 * 60);

/// One iteration of the periodic-scan loop: read the configured interval,
/// and when set, post [`Task::Scan`] / [`Task::ScanAudiobooks`] for each
/// configured library path via `worker` — the same tasks a manual "Scan
/// Library" click or a settings save would post. Returns how long the
/// caller should sleep before the next tick: the full configured interval
/// (converted to seconds) once a scan has been posted, or
/// [`PERIODIC_SCAN_RECHECK`] when the interval is unset so a later settings
/// change takes effect without a restart.
pub async fn periodic_scan_tick(pool: &SqlitePool, worker: &Arc<Worker>) -> Duration {
    let settings = match crate::settings::get_settings(pool).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "periodic scan: failed to read settings");
            return PERIODIC_SCAN_RECHECK;
        }
    };
    let Some(hours) = settings.scan_interval_hours else {
        return PERIODIC_SCAN_RECHECK;
    };
    if let Some(library_path) = settings.ebook_library_path {
        worker.post(Task::Scan { library_path });
    }
    if let Some(library_path) = settings.audiobook_library_path {
        worker.post(Task::ScanAudiobooks { library_path });
    }
    Duration::from_secs(u64::from(hours) * 3600)
}
