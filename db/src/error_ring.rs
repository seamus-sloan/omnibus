//! Process-wide, bounded ring buffer of captured ERROR-level tracing events.
//!
//! Populated by the tracing `Layer` installed in `server::logging::init_tracing`
//! (no explicit call site at each error) and read by the admin-gated
//! `rpc_get_last_errors` server function. A fast, in-memory companion to the
//! durable on-disk JSON log sink (`crate::logs`) — not a replacement: lost on
//! restart, by design.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

pub use omnibus_shared::error_ring::{CapturedError, ERROR_RING_CAPACITY};

#[cfg(test)]
mod tests;

/// The process-wide buffer, initialized lazily on first access. A plain
/// `std::sync::Mutex` is enough — writes and reads are both cheap (push/pop
/// on a bounded `VecDeque`), so there's no reason to reach for an `RwLock`.
static BUFFER: OnceLock<Mutex<VecDeque<CapturedError>>> = OnceLock::new();

fn buffer() -> &'static Mutex<VecDeque<CapturedError>> {
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(ERROR_RING_CAPACITY)))
}

/// Recover from a poisoned `std::sync::Mutex` instead of panicking, mirroring
/// `worker::lock_unpoison`. The guarded data is just an in-memory list with
/// no invariant a partial write could break, so taking the inner guard and
/// carrying on is strictly safer than a process-wide crash cascading from
/// one thread's unrelated panic.
fn lock_unpoison<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Record one captured error, evicting the oldest entry first once the
/// buffer is at [`ERROR_RING_CAPACITY`] (AC1). Never called directly at
/// error call sites — only the tracing `Layer` calls this.
pub fn record(entry: CapturedError) {
    let mut buf = lock_unpoison(buffer());
    if buf.len() >= ERROR_RING_CAPACITY {
        buf.pop_front();
    }
    buf.push_back(entry);
}

/// Snapshot the buffer's current contents, newest first.
pub fn snapshot() -> Vec<CapturedError> {
    let buf = lock_unpoison(buffer());
    buf.iter().rev().cloned().collect()
}

/// Clear the buffer. Test-only: tests share the one process-wide buffer, so
/// each test that asserts on its contents must reset it first. Gated the
/// same way as `test_support` (not just `cfg(test)`) so the cross-crate
/// tracing-`Layer` tests in `server/` — which depend on this crate with
/// `feature = "test-support"` — can reach it via `test_support::clear_error_ring`.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn clear() {
    lock_unpoison(buffer()).clear();
}
