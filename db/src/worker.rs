//! Background-worker primitive: a single-process queue with per-kind
//! concurrency semaphores (library scan, HLS transcode, format convert) and a
//! per-resource keyed mutex map that serializes tasks sharing a resource key,
//! so two scans of one library path queue behind each other while different
//! paths run in parallel. Split across the sub-modules declared below.

mod exec;
mod handlers;
mod metrics;
mod periodic_scan;
mod progress;
mod queue;
mod types;

#[cfg(test)]
mod tests;

pub use metrics::WorkerMetrics;
pub use periodic_scan::{periodic_scan_tick, PERIODIC_SCAN_RECHECK};
pub use types::{Task, TaskId, TaskOutcome, TaskSuccessDetail, Worker, WorkerConfig};
