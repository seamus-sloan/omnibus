//! Durable history of background worker task runs (issue #941).
//!
//! The `background_tasks` table (migration `0072`) is the on-disk companion
//! to `worker::types::Worker`'s in-memory `progress` map: `start_task` is
//! called once a task begins, `finish_task` once it reaches a terminal
//! state, and `recent_tasks` backs the admin dashboard. Mirrors how
//! `error_ring` (fast, in-memory) pairs with the durable `logs` sink.

use sqlx::{Row, SqlitePool};

pub use omnibus_shared::worker::{BackgroundTaskRecord, BackgroundTaskStatus};

/// Column list shared by every row-returning query, matched by [`row_to_record`].
const COLS: &str = "id, task_kind, status, started_at, finished_at, error";

/// Failure space for background-task persistence queries. A single
/// transparent variant: every caller here just propagates or logs, none
/// branches on the failure mode (`.claude/rules/02-error-handling.md`).
#[derive(Debug, thiserror::Error)]
pub enum BackgroundTaskError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Insert a `running` row for a task that just started, returning its
/// autoincrement id — the handle a later [`finish_task`] call updates.
pub async fn start_task(
    pool: &SqlitePool,
    task_kind: &str,
    started_at: i64,
) -> Result<i64, BackgroundTaskError> {
    let row = sqlx::query(
        "INSERT INTO background_tasks (task_kind, status, started_at) \
         VALUES (?, 'running', ?) RETURNING id",
    )
    .bind(task_kind)
    .bind(started_at)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Update a task row to its terminal state, recording the finish time and
/// (only on failure) the error text. A no-op — not an error — if `id` no
/// longer exists; the row is best-effort observability, not a write the
/// worker's own success depends on.
pub async fn finish_task(
    pool: &SqlitePool,
    id: i64,
    status: BackgroundTaskStatus,
    finished_at: i64,
    error: Option<&str>,
) -> Result<(), BackgroundTaskError> {
    sqlx::query("UPDATE background_tasks SET status = ?, finished_at = ?, error = ? WHERE id = ?")
        .bind(status.as_db_str())
        .bind(finished_at)
        .bind(error)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Fetch the most recent `limit` task rows, newest first — the admin
/// dashboard's whole read pattern (AC3).
pub async fn recent_tasks(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<BackgroundTaskRecord>, BackgroundTaskError> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM background_tasks ORDER BY id DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// Map one `background_tasks` row to its wire record. The `status` column is
/// constrained by the migration's `CHECK` to the three values
/// [`BackgroundTaskStatus::from_str`] parses, so a row written through this
/// module can never fail to parse. A mismatch here means something else
/// wrote the row — a future version with a status value this build doesn't
/// know, or manual DB surgery — so it's logged rather than silently
/// swallowed, while still falling back to `Running` so the dashboard read
/// never hard-fails on one corrupted row.
fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> BackgroundTaskRecord {
    let id: i64 = row.get("id");
    let status_raw: String = row.get("status");
    let status = status_raw.parse().unwrap_or_else(|_| {
        tracing::warn!(
            id,
            status = %status_raw,
            "background_tasks: row has an unrecognized status, defaulting to Running"
        );
        BackgroundTaskStatus::Running
    });
    BackgroundTaskRecord {
        id,
        task_kind: row.get("task_kind"),
        status,
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        error: row.get("error"),
    }
}

#[cfg(test)]
mod tests;
