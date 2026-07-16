//! Testable core of the periodic library-rescan background task: reads the
//! configured interval, posts scan tasks when due, and returns the next sleep.
//! Called on a schedule by `server::main`'s `spawn_periodic_scan` loop.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use omnibus_shared::settings::SCAN_INTERVAL_MIN_HOURS;

use super::types::{Task, Worker};

#[cfg(test)]
mod tests;

/// Recheck cadence used while `scan_interval_hours` is unset (or a settings
/// read fails), so enabling the setting later takes effect without a
/// restart instead of waiting on a timer that was never armed.
pub const PERIODIC_SCAN_RECHECK: Duration = Duration::from_secs(5 * 60);

/// One iteration of the periodic-scan loop: read the configured interval,
/// and when it's set and at least [`SCAN_INTERVAL_MIN_HOURS`], post
/// [`Task::Scan`] / [`Task::ScanAudiobooks`] for each configured library
/// path via `worker` — the same tasks a manual "Scan Library" click or a
/// settings save would post. Returns how long the caller should sleep
/// before the next tick.
///
/// The returned duration is [`PERIODIC_SCAN_RECHECK`] — never
/// [`Duration::ZERO`] or a sub-recheck sleep — in every case where a scan
/// was *not* posted, so the caller's loop can't busy-spin and a later
/// settings change is always picked up within the recheck window:
/// - the settings read failed;
/// - the interval is unset (disabled);
/// - the interval is below [`SCAN_INTERVAL_MIN_HOURS`] (defense-in-depth
///   against a corrupted/hand-edited `settings` row that parses as
///   `Some(0)` — `Settings::validate` already rejects this on the write
///   path, but `get_settings` doesn't re-validate on read);
/// - the interval is valid but no library path is configured yet.
///
/// Only when a scan is actually posted does it return the full configured
/// interval.
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
    if hours < SCAN_INTERVAL_MIN_HOURS {
        tracing::warn!(
            hours,
            "periodic scan: interval below minimum, treating as disabled"
        );
        return PERIODIC_SCAN_RECHECK;
    }
    let mut posted = false;
    if let Some(library_path) = settings.ebook_library_path {
        worker.post(Task::Scan { library_path });
        posted = true;
    }
    if let Some(library_path) = settings.audiobook_library_path {
        worker.post(Task::ScanAudiobooks { library_path });
        posted = true;
    }
    // Enabled but nothing to scan yet: recheck soon so a path added later
    // takes effect within the recheck window instead of after a full interval.
    if !posted {
        return PERIODIC_SCAN_RECHECK;
    }
    Duration::from_secs(u64::from(hours) * 3600)
}
