//! Background-worker primitive.
//!
//! Single-process queue with two fairness knobs:
//! - `scan_concurrency` caps how many `Task::Scan` / `Task::ScanAudiobooks`
//!   jobs run concurrently (acquired from a per-Worker scan [`Semaphore`]).
//! - `hls_concurrency` caps concurrent `Task::HlsTranscode` jobs (acquired
//!   from a separate HLS [`Semaphore`]).
//! - A per-resource keyed mutex map serializes any tasks that share the
//!   same resource key, so e.g. two scans of the same library path queue
//!   behind each other while different paths run in parallel.
//!
//! Split across focused sub-modules:
//!
//! * [`types`] — `Task` / `TaskOutcome` / `WorkerConfig` / `Worker` struct
//!   and shared helpers (`lock_unpoison`, `wall_clock_ms`).
//! * [`queue`] — `Worker::post` / `await_completion` + RAII map-cleanup
//!   guards.
//! * [`exec`] — `Worker::run` dispatch loop: resource lock + scan
//!   semaphore + terminal-state projection.
//! * [`handlers`] — `Worker::execute` per-task-kind handlers.
//! * [`progress`] — `progress_snapshot` + retention/eviction +
//!   `report_progress` + the terminal-state writer.

mod exec;
mod handlers;
mod progress;
mod queue;
mod types;

#[cfg(test)]
mod tests;

pub use types::{Task, TaskId, TaskOutcome, Worker, WorkerConfig};
