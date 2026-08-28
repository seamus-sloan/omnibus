//! Annual reading goals: the stored `reading_goals` target for one
//! `(user, year, kind)`, paired with progress measured by the same completion
//! definition every other stats surface uses. Read by `compute` onto
//! [`StatsSummary::goal`]; written by the `PUT /api/stats/goal` handler and
//! the web RPC, both of which come through [`set_goal`].

use omnibus_shared::{
    ReadingGoal, ReadingGoalUpdate, GOAL_KIND_BOOKS, MAX_GOAL_TARGET, MAX_GOAL_YEAR, MIN_GOAL_YEAR,
};
use sqlx::{Row, SqlitePool};

use super::{compute, StatsError};

/// Failure space of the goal write path. Every variant but `Sqlx` is a
/// boundary check the handler renders as a 400, so callers branch on them.
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("unsupported goal kind: {0}")]
    UnsupportedKind(String),
    #[error("target must be between 1 and {MAX_GOAL_TARGET}: got {0}")]
    InvalidTarget(i64),
    #[error("year must be between {MIN_GOAL_YEAR} and {MAX_GOAL_YEAR}: got {0}")]
    InvalidYear(i64),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<StatsError> for GoalError {
    fn from(e: StatsError) -> Self {
        match e {
            StatsError::Sqlx(inner) => Self::Sqlx(inner),
        }
    }
}

/// The server's current calendar year (UTC). Read from SQLite rather than the
/// process clock so it lands on the same "now" every other stats window does.
pub async fn current_year(pool: &SqlitePool) -> Result<i64, StatsError> {
    Ok(
        sqlx::query_scalar("SELECT CAST(strftime('%Y', 'now') AS INTEGER)")
            .fetch_one(pool)
            .await?,
    )
}

/// Unix-second bounds `[start, end)` of `year` in UTC.
async fn year_bounds(pool: &SqlitePool, year: i64) -> Result<(i64, i64), StatsError> {
    let row = sqlx::query(
        "SELECT CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) AS lo,
                CAST(strftime('%s', ? || '-01-01 00:00:00') AS INTEGER) AS hi",
    )
    .bind(format!("{year:04}"))
    .bind(format!("{:04}", year + 1))
    .fetch_one(pool)
    .await?;
    Ok((row.try_get("lo")?, row.try_get("hi")?))
}

/// The caller's goal for the current calendar year, `None` when unset — the
/// value `compute` hangs off [`omnibus_shared::StatsSummary::goal`].
pub(super) async fn current_goal(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<ReadingGoal>, StatsError> {
    let year = current_year(pool).await?;
    goal_for_year(pool, user_id, year).await
}

/// The caller's goal for `year`, paired with that year's completion count.
///
/// The count is bounded to `[Jan 1 year, Jan 1 year+1)` via
/// [`compute::finished_count_bounded`], the same helper the drill-in's
/// vs-previous delta uses — so a goal filed against a past year reports that
/// year's real total and setting it changes no count anywhere.
pub async fn goal_for_year(
    pool: &SqlitePool,
    user_id: i64,
    year: i64,
) -> Result<Option<ReadingGoal>, StatsError> {
    let Some(row) = sqlx::query(
        "SELECT kind, target FROM reading_goals WHERE user_id = ? AND year = ? AND kind = ?",
    )
    .bind(user_id)
    .bind(year)
    .bind(GOAL_KIND_BOOKS)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let (start, end) = year_bounds(pool, year).await?;
    let current = compute::finished_count_bounded(pool, user_id, start, end).await?;
    Ok(Some(ReadingGoal {
        kind: row.try_get("kind")?,
        target: row.try_get("target")?,
        current,
        year,
    }))
}

/// Set, change, or clear the caller's goal.
///
/// An absent `target` clears the row — there is no separate delete path, so
/// "no goal" stays a missing row rather than a zero target the ring would
/// divide by. `year` defaults to the server's current year so no client bakes
/// its own clock into the write.
///
/// Invalidates this user's cached summaries before returning: the aggregate
/// cache is keyed `(user_id, range)` with a 60s TTL, and a just-saved goal
/// reading stale is worse than a stale count.
pub async fn set_goal(
    pool: &SqlitePool,
    user_id: i64,
    update: &ReadingGoalUpdate,
) -> Result<Option<ReadingGoal>, GoalError> {
    let kind = update.kind_or_default();
    if kind != GOAL_KIND_BOOKS {
        return Err(GoalError::UnsupportedKind(kind.to_string()));
    }
    let year = match update.year {
        Some(year) => year,
        None => current_year(pool).await?,
    };
    if !(MIN_GOAL_YEAR..=MAX_GOAL_YEAR).contains(&year) {
        return Err(GoalError::InvalidYear(year));
    }

    match update.target {
        Some(target) => {
            if !(1..=MAX_GOAL_TARGET).contains(&target) {
                return Err(GoalError::InvalidTarget(target));
            }
            sqlx::query(
                "INSERT INTO reading_goals (user_id, year, kind, target, updated_at)
                 VALUES (?, ?, ?, ?, strftime('%s','now'))
                 ON CONFLICT(user_id, year, kind) DO UPDATE SET
                     target = excluded.target,
                     updated_at = strftime('%s','now')",
            )
            .bind(user_id)
            .bind(year)
            .bind(kind)
            .bind(target)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM reading_goals WHERE user_id = ? AND year = ? AND kind = ?")
                .bind(user_id)
                .bind(year)
                .bind(kind)
                .execute(pool)
                .await?;
        }
    }

    super::invalidate_user(user_id);
    Ok(goal_for_year(pool, user_id, year).await?)
}

#[cfg(test)]
mod tests;
