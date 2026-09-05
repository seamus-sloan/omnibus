//! Integration tests for the worker primitive, split by sub-topic into the
//! sibling modules below; the pool and default-worker fixtures they share
//! live here. Concurrency, resource serialization, panic / poison recovery
//! and the progress-snapshot eviction window are the acceptance gates for
//! the worker submodule split.

mod lifecycle;
mod metrics;
mod poison;
mod progress;
mod scan;
mod tasks;

use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use super::types::{Worker, WorkerConfig};

async fn pool() -> SqlitePool {
    crate::init_db("sqlite::memory:").await.unwrap()
}

fn make_worker_default(pool: SqlitePool) -> Arc<Worker> {
    Worker::new(pool, WorkerConfig::default())
}

/// Poll until both worker maps are empty or a deadline elapses, so
/// fire-and-forget assertions don't hinge on a fixed sleep. Returns
/// whether the maps drained in time.
async fn poll_maps_empty(w: &Arc<Worker>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if w.completions_len() == 0 && w.resource_locks_len().await == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
