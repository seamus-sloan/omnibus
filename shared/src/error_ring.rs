//! Wire types for the in-memory "last errors" ring buffer.
//!
//! Populated by the tracing `Layer` installed in `server::logging::init_tracing`
//! from ERROR-level events; read by the admin-gated `rpc_get_last_errors`
//! server function that feeds the Settings → Server Health "Last errors"
//! section. A fast, bounded, in-memory companion to the durable JSON log
//! sink (`omnibus_shared::logs`) — not a replacement for it.

use serde::{Deserialize, Serialize};

/// Max entries retained by the ring buffer (`omnibus_db::error_ring`),
/// oldest evicted first.
pub const ERROR_RING_CAPACITY: usize = 200;

/// One captured ERROR-level tracing event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CapturedError {
    /// Unix-seconds timestamp when the event was captured.
    pub timestamp: i64,
    /// Event target — the emitting module path.
    pub target: String,
    /// The event's `message` field, or empty when it carried none.
    pub message: String,
    /// The book uuid the event named, if any (a `book_uuid` or `uuid`
    /// field), letting the UI link straight to `/books/{uuid}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_uuid: Option<String>,
    /// The file path the event named, if any (a `file`, `path`, or
    /// `file_path` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}
