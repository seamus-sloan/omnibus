//! Worker progress feed wire types shared with the frontend.
//!
//! Mirrors the payload-less shape of the server-side `Task` enum so the wire
//! protocol stays decoupled from the runtime `Task` type's lifetimes and
//! handler closures.

use serde::{Deserialize, Serialize};

/// Discriminant for [`TaskProgress`] entries. Mirrors the payload-less
/// shape of the server-side `Task` enum so the wire protocol stays
/// decoupled from the runtime `Task` type's lifetimes and handler
/// closures. `#[non_exhaustive]` so future worker actions can be added
/// without breaking client matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskKind {
    Scan,
    GenerateThumbs,
    ResolveAuthorPhoto,
    RefetchAuthorPhotos,
}

/// Lifecycle state of a single worker task as exposed to the UI.
///
/// `Running.total = None` means the task either has no granular progress
/// surface yet or is in a pre-count phase (e.g. scanner tree-walk). When
/// `total` is `Some`, `processed / total` is a stable ratio the UI can
/// render as a progress bar. Terminal variants (`Done`, `Failed`) stick
/// around in [`WorkerStatus::recent_complete`] for ~10s after completion
/// so a transient "Library updated" / error banner can render before the
/// indicator collapses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProgressState {
    Running {
        processed: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u32>,
    },
    Done {
        processed: u32,
    },
    Failed {
        message: String,
    },
}

impl ProgressState {
    /// Terminal states (`Done`, `Failed`) live in the "recently completed"
    /// bucket; non-terminal states live in "active." Used by both the worker
    /// snapshot partitioner and the client renderer.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProgressState::Done { .. } | ProgressState::Failed { .. }
        )
    }
}

/// One row of the worker progress feed. `task_id` is the process-local
/// worker id (not stable across server restarts); the UI uses it only as a
/// stable key for list rendering and dismiss-tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskProgress {
    pub task_id: u64,
    pub kind: TaskKind,
    pub state: ProgressState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
    /// Milliseconds since the UNIX epoch — wall-clock, for UI elapsed-time
    /// display only. The server's actual eviction logic uses a monotonic
    /// `Instant` internally, so these timestamps don't have to be precise.
    pub started_at_ms: i64,
    pub last_update_ms: i64,
}

/// Aggregate progress feed served from `POST /api/rpc/worker_status`.
///
/// Two-vec split lets callers short-circuit the "do I need a fade timer?"
/// check by inspecting `recent_complete.is_empty()` without scanning
/// `active`. Both vecs are sorted by `task_id` for stable list rendering
/// across polls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStatus {
    pub active: Vec<TaskProgress>,
    pub recent_complete: Vec<TaskProgress>,
}

impl WorkerStatus {
    /// `true` when no tasks are active or recently completed.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.recent_complete.is_empty()
    }
}
