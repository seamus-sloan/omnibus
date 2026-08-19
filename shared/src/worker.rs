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
    BackfillChapters,
    ResolveSuggestions,
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
        /// Set when a scan ghosted a large-but-sub-abort-threshold number of
        /// books — the UI renders a dismissible warning row
        /// instead of the ordinary success row.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ghost_warning: Option<GhostFilesWarning>,
        /// `book_uuid`s that failed a fleet-wide EPUB override bake — `Some`
        /// only for a `RewriteAllEpubs` run that left at
        /// least one book unbaked; every other task kind, and a bake with
        /// zero failures, reports `None`. Mutually exclusive with
        /// `ghost_warning` (each task kind produces at most one of the two).
        /// Deliberately just the uuids, not the per-book error text: this
        /// endpoint is reachable by any authenticated user, not only the
        /// admin who kicked off the bake, and the underlying error (from
        /// `db::epub_rewrite`) can carry a server filesystem path. The full
        /// message is logged server-side instead — see
        /// `handle_rewrite_all_epubs` in `db/src/worker/handlers.rs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bake_errors: Option<Vec<String>>,
    },
    Failed {
        message: String,
    },
}

/// Ghost-count tally attached to a scan's `Done` state: `removed` books lost
/// their backing file this scan out of `total` file-backed books in the
/// library, clearing the warn threshold but staying under the abort guard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhostFilesWarning {
    pub removed: u32,
    pub total: u32,
}

/// Live verbose detail attached to a [`TaskProgress`] entry: which phase the
/// task is in, the item it is working on right now, and (for scans) the
/// diff's classification tallies. Every field is optional so task kinds
/// opt in piecemeal; all three absent serializes to no `detail` key at all.
///
/// `current_item` is user-facing display text and must never carry an
/// absolute server path — scans report the library directory name plus the
/// library-relative path (`books/Author/Title.epub`), other tasks report a
/// book title or author name. `rpc_worker_status` is readable by any
/// authenticated user, so this is a security boundary, not a style choice.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetail {
    /// Short human-readable phase label, e.g. "Walking the library".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The item being processed right now, e.g. "books/Author/Title.epub".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_item: Option<String>,
    /// Scan classification tallies; retained on the terminal `Done` entry so
    /// the completion row can summarize what the scan changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tallies: Option<ScanTallies>,
}

impl TaskDetail {
    /// `true` when no field carries anything — the "render nothing" state.
    pub fn is_empty(&self) -> bool {
        self.phase.is_none() && self.current_item.is_none() && self.tallies.is_none()
    }
}

/// Running tallies from a library scan's diff: how many files the walk
/// `found`, and how the diff classified them. Counts are fixed once the
/// diff completes; the UI renders them alongside the per-file progress.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanTallies {
    pub found: u32,
    pub new: u32,
    pub changed: u32,
    pub removed: u32,
    pub moved: u32,
    pub unchanged: u32,
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
    /// Live phase / current-item / tally detail. `None` for task kinds that
    /// haven't opted in, and omitted from the wire entirely in that case so
    /// pre-existing payload shapes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<TaskDetail>,
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

/// Lifecycle status of a persisted [`BackgroundTaskRecord`].
/// `Running` covers a task that started but has no terminal write yet —
/// including one an ungraceful shutdown left stuck, which is itself a
/// useful signal on the admin dashboard rather than something to hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Success,
    Failed,
}

impl BackgroundTaskStatus {
    /// The DB `status` column's TEXT representation — also the CHECK
    /// constraint's allowed set (migration `0072`).
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for BackgroundTaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown background task status: {other}")),
        }
    }
}

/// One durable row of `background_tasks` history: a worker task
/// run's kind, lifecycle status, and timing. Read-only from the wire's
/// perspective — the admin dashboard only ever fetches these, never writes
/// them (the worker is the sole writer, via `omnibus_db::background_tasks`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundTaskRecord {
    pub id: i64,
    pub task_kind: String,
    pub status: BackgroundTaskStatus,
    /// Unix-seconds.
    pub started_at: i64,
    /// Unix-seconds; `None` while `status` is `Running`.
    pub finished_at: Option<i64>,
    /// Set only when `status` is `Failed`. Client-facing — the worker's
    /// `handlers::sanitized_err` convention already keeps raw internals out
    /// of every `TaskOutcome::Err`, and this carries that same sanitized text.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_without_a_ghost_warning_omits_the_field_from_the_wire() {
        // AC5: the wire type must stay compact for the ordinary (no
        // warning) case — no `ghost_warning` key at all, not even `null`.
        let state = ProgressState::Done {
            processed: 3,
            ghost_warning: None,
            bake_errors: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("ghost_warning"),
            "expected no ghost_warning key, got {json}"
        );
    }

    #[test]
    fn done_with_a_ghost_warning_round_trips_the_removed_and_total_counts() {
        let state = ProgressState::Done {
            processed: 100,
            ghost_warning: Some(GhostFilesWarning {
                removed: 15,
                total: 100,
            }),
            bake_errors: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let round_tripped: ProgressState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, round_tripped);
    }

    #[test]
    fn done_without_bake_errors_omits_the_field_from_the_wire() {
        // Mirrors `done_without_a_ghost_warning_omits_the_field_from_the_wire`
        // (#1739): the wire type must stay compact for the ordinary
        // all-succeeded bake case too.
        let state = ProgressState::Done {
            processed: 3,
            ghost_warning: None,
            bake_errors: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("bake_errors"),
            "expected no bake_errors key, got {json}"
        );
    }

    #[test]
    fn done_with_bake_errors_round_trips_the_book_uuids() {
        let state = ProgressState::Done {
            processed: 2,
            ghost_warning: None,
            bake_errors: Some(vec!["uuid-bad".into()]),
        };
        let json = serde_json::to_string(&state).unwrap();
        let round_tripped: ProgressState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, round_tripped);
    }

    #[test]
    fn task_progress_without_detail_omits_the_field_from_the_wire() {
        let progress = TaskProgress {
            task_id: 1,
            kind: TaskKind::Scan,
            state: ProgressState::Running {
                processed: 0,
                total: None,
            },
            resource_key: None,
            detail: None,
            started_at_ms: 0,
            last_update_ms: 0,
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert!(
            !json.contains("detail"),
            "expected no detail key, got {json}"
        );
    }

    #[test]
    fn task_progress_with_detail_round_trips_phase_item_and_tallies() {
        let progress = TaskProgress {
            task_id: 1,
            kind: TaskKind::Scan,
            state: ProgressState::Running {
                processed: 3,
                total: Some(10),
            },
            resource_key: None,
            detail: Some(TaskDetail {
                phase: Some("Reading file metadata".into()),
                current_item: Some("books/Author/Title.epub".into()),
                tallies: Some(ScanTallies {
                    found: 10,
                    new: 3,
                    changed: 1,
                    removed: 2,
                    moved: 1,
                    unchanged: 3,
                }),
            }),
            started_at_ms: 0,
            last_update_ms: 0,
        };
        let json = serde_json::to_string(&progress).unwrap();
        let round_tripped: TaskProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(progress, round_tripped);
    }

    #[test]
    fn task_detail_is_empty_only_when_every_field_is_absent() {
        assert!(TaskDetail::default().is_empty());
        assert!(!TaskDetail {
            phase: Some("Walking the library".into()),
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn done_with_bake_errors_never_carries_a_message_key_on_the_wire() {
        // Regression guard for the PR #1756 review fix: the wire payload
        // must be uuid-only — no per-book error text, which can carry a
        // server filesystem path and is reachable by any authenticated user
        // via `rpc_worker_status`, not only the admin who ran the bake.
        let state = ProgressState::Done {
            processed: 1,
            ghost_warning: None,
            bake_errors: Some(vec!["uuid-bad".into()]),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("message"),
            "bake_errors must not carry a message field: {json}"
        );
    }
}
